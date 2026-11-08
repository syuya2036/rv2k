use crate::syscall::{raw, Error, Number};

/// Nonblocking UART output. Success is the number copied into the OS ring.
pub fn write(bytes: &[u8]) -> Result<usize, Error> {
    unsafe {
        Error::decode(raw(
            Number::Write as usize,
            [1, bytes.as_ptr() as usize, bytes.len()],
        ))
    }
}
