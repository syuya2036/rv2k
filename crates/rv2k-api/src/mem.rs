/// Fixed, versioned memory report; all sizes include padding/guards.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemInfo {
    pub version: u16,
    pub static_bytes: u16,
    pub kernel_reserved: u16,
    pub task_reserved: u16,
    pub used: [u16; 3],
    pub guard_failures: u16,
    pub flash_bytes: u16,
    pub ram_limit: u16,
    pub flash_limit: u16,
    pub reserved: u16,
}
const _: () = assert!(core::mem::size_of::<MemInfo>() == 24);
impl MemInfo {
    /// Visit the little-endian wire image without a second stack buffer on RV32.
    pub fn with_bytes<T>(&self, visit: impl FnOnce(&[u8]) -> T) -> T {
        #[cfg(target_endian = "little")]
        {
            // repr(C), twelve u16 values and size=24 prove there is no padding.
            // Every byte is initialized; this shared view cannot outlive self.
            visit(unsafe { core::slice::from_raw_parts((self as *const Self).cast::<u8>(), 24) })
        }
        #[cfg(target_endian = "big")]
        {
            visit(&self.encode())
        }
    }
    pub fn encode(&self) -> [u8; 24] {
        let values = [
            self.version,
            self.static_bytes,
            self.kernel_reserved,
            self.task_reserved,
            self.used[0],
            self.used[1],
            self.used[2],
            self.guard_failures,
            self.flash_bytes,
            self.ram_limit,
            self.flash_limit,
            self.reserved,
        ];
        let mut out = [0; 24];
        for (index, value) in values.iter().enumerate() {
            out[index * 2..index * 2 + 2].copy_from_slice(&value.to_le_bytes());
        }
        out
    }
}

#[cfg(target_arch = "riscv32")]
pub fn info() -> Result<MemInfo, crate::syscall::Error> {
    with_info(|info| *info)
}

/// Visit one copied report without a second aggregate return buffer. The view
/// is task-local and remains valid if the callback waits or is preempted.
#[cfg(target_arch = "riscv32")]
pub fn with_info<T>(read: impl FnOnce(&MemInfo) -> T) -> Result<T, crate::syscall::Error> {
    use crate::syscall::{raw, Error, Number};
    let mut info = core::mem::MaybeUninit::<MemInfo>::uninit();
    unsafe {
        Error::decode(raw(
            Number::MemInfo as usize,
            [info.as_mut_ptr() as usize, 24, 0],
        ))?;
        Ok(read(info.assume_init_ref()))
    }
}
