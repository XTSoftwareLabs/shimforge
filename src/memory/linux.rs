use super::{PageRange, Region, os_error, syscall};
use crate::Error;

mod maps;

pub(super) fn region_at(address: usize) -> Result<Option<Region>, Error> {
    let maps = syscall(
        "query memory",
        Err(std::io::Error::from_raw_os_error(libc::EIO)),
        maps::read,
    )
    .map_err(|error| Error::Mapping(error.to_string()))?;
    parse_region(&maps, address)
}

fn parse_region(maps: &str, address: usize) -> Result<Option<Region>, Error> {
    for line in maps.lines() {
        let mut columns = line.split_whitespace();
        let range = columns.next().unwrap_or("");
        let permissions = columns.next().unwrap_or("").as_bytes();
        let (start, end) = range
            .split_once('-')
            .ok_or_else(|| Error::Mapping("missing mapping address range".into()))?;
        let start =
            usize::from_str_radix(start, 16).map_err(|error| Error::Mapping(error.to_string()))?;
        let end =
            usize::from_str_radix(end, 16).map_err(|error| Error::Mapping(error.to_string()))?;
        if start >= end
            || permissions.len() != 4
            || !matches!(permissions[0], b'r' | b'-')
            || !matches!(permissions[1], b'w' | b'-')
            || !matches!(permissions[2], b'x' | b'-')
            || !matches!(permissions[3], b'p' | b's')
        {
            return Err(Error::Mapping(
                "invalid mapping permissions or range".into(),
            ));
        }
        if start <= address && address < end {
            let mut protection = libc::PROT_NONE;
            if permissions[0] == b'r' {
                protection |= libc::PROT_READ;
            }
            if permissions[1] == b'w' {
                protection |= libc::PROT_WRITE;
            }
            if permissions[2] == b'x' {
                protection |= libc::PROT_EXEC;
            }
            return Ok(Some(Region {
                start,
                end,
                protection: protection as u32,
                readable: permissions[0] == b'r',
                executable: permissions[2] == b'x',
            }));
        }
    }
    Ok(None)
}

pub(super) fn read_bytes(address: usize, length: usize) -> Result<Vec<u8>, Error> {
    let mut bytes = vec![0; length];
    let local = libc::iovec {
        iov_base: bytes.as_mut_ptr().cast(),
        iov_len: length,
    };
    let remote = libc::iovec {
        iov_base: address as *mut libc::c_void,
        iov_len: length,
    };
    // SAFETY: local points to the output buffer. The kernel checks the source address.
    let copied = syscall("read memory", -1, || unsafe {
        libc::process_vm_readv(libc::getpid(), &local, 1, &remote, 1, 0)
    });
    if copied < 0 {
        return Err(os_error("read memory"));
    }
    bytes.truncate(copied as usize);
    Ok(bytes)
}

pub(super) fn page_size() -> Result<usize, Error> {
    // SAFETY: sysconf reads a system setting and takes no pointers.
    let size = syscall("page size", -1, || unsafe {
        libc::sysconf(libc::_SC_PAGESIZE)
    });
    usize::try_from(size).map_err(|_| os_error("page size"))
}

pub(super) fn protect(page: PageRange, writable: bool) -> Result<(), Error> {
    let protection = page.protection as i32 | if writable { libc::PROT_WRITE } else { 0 };
    // SAFETY: The caller keeps these aligned pages mapped. Permissions came from /proc.
    let result = syscall("protect memory", -1, || unsafe {
        libc::mprotect(page.address as *mut libc::c_void, page.length, protection)
    });
    if result != 0 {
        return Err(os_error("protect memory"));
    }
    Ok(())
}

pub(super) fn flush(_address: usize, _length: usize) -> Result<(), Error> {
    #[cfg(test)]
    if super::tests::should_fail("flush instruction cache") {
        return Err(Error::Os {
            operation: "flush instruction cache",
            code: libc::EIO,
        });
    }
    // x86-64 keeps code and data caches in sync. Target calls are stopped during writes.
    std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

#[cfg(test)]
mod tests;
