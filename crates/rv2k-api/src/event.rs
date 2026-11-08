pub const NET_RX: u32 = 1;
pub const NET_TX_READY: u32 = 1 << 1;
pub const NET_CONNECTED: u32 = 1 << 2;
pub const NET_CLOSED: u32 = 1 << 3;
pub const MSG_AVAILABLE: u32 = 1 << 4;
pub const DRIVER_POLL: u32 = 1 << 5;
pub const MSG_SPACE: u32 = 1 << 6;
pub const ALL: u32 = (1 << 7) - 1;
pub const FOREVER: u32 = u32::MAX;

#[cfg(target_arch = "riscv32")]
pub fn wait(mask: u32, timeout: u32) -> Result<u32, crate::syscall::Error> {
    use crate::syscall::{raw, Error, Number};
    unsafe {
        Error::decode(raw(
            Number::EventWait as usize,
            [mask as usize, timeout as usize, 0],
        ))
        .map(|bits| bits as u32)
    }
}

#[cfg(target_arch = "riscv32")]
pub fn signal(id: crate::TaskId, mask: u32) -> Result<(), crate::syscall::Error> {
    use crate::syscall::{raw, Error, Number};
    unsafe {
        Error::decode(raw(
            Number::EventSignal as usize,
            [id.index(), mask as usize, 0],
        ))
        .map(|_| ())
    }
}
