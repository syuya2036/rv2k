use crate::syscall::{raw, Error, Number};

pub fn sleep(ticks: u32) -> Result<(), Error> {
    unsafe { Error::decode(raw(Number::Sleep as usize, [ticks as usize, 0, 0])).map(|_| ()) }
}

pub fn uptime() -> Result<u64, Error> {
    let mut value = 0u64;
    unsafe {
        Error::decode(raw(
            Number::Uptime as usize,
            [&mut value as *mut u64 as usize, 8, 0],
        ))?;
    }
    Ok(value)
}
