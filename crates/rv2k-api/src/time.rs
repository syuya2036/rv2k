use crate::syscall::{raw, Error, Number};

pub fn sleep(ticks: u32) -> Result<(), Error> {
    unsafe { Error::decode(raw(Number::Sleep as usize, [ticks as usize, 0, 0])).map(|_| ()) }
}

#[inline(always)]
pub fn uptime() -> Result<u64, Error> {
    let mut value = core::mem::MaybeUninit::<u64>::uninit();
    unsafe {
        Error::decode(raw(
            Number::Uptime as usize,
            [value.as_mut_ptr() as usize, 8, 0],
        ))?;
    }
    // A successful uptime syscall initializes all eight output bytes.
    Ok(unsafe { value.assume_init() })
}
