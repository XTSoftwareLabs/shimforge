use super::super::syscall;
use std::io;

struct File(libc::c_long);

impl Drop for File {
    fn drop(&mut self) {
        // SAFETY: this descriptor belongs to us. Linux close is not retried.
        unsafe { libc::syscall(libc::SYS_close, self.0) };
    }
}

pub(super) fn read() -> io::Result<String> {
    let file = File(retry(&mut || {
        // SAFETY: the path is NUL-terminated; no file is created or changed.
        syscall("open maps", -1, || unsafe {
            libc::syscall(
                libc::SYS_openat,
                libc::AT_FDCWD,
                c"/proc/self/maps".as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC,
                0,
            )
        })
    })?);
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let count = retry(&mut || {
            // SAFETY: the descriptor is live and the buffer is writable.
            syscall("read maps", -1, || unsafe {
                libc::syscall(libc::SYS_read, file.0, buffer.as_mut_ptr(), buffer.len())
            })
        })? as usize;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    // File names may contain non-UTF-8 bytes; the mapping fields are ASCII.
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn retry(call: &mut dyn FnMut() -> libc::c_long) -> io::Result<libc::c_long> {
    loop {
        let result = call();
        if result >= 0 {
            return Ok(result);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(test)]
mod tests;
