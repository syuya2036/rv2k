//! Test-only MMIO NIC. Frame storage is on the emulated device, never MCU RAM.
use crate::network::{DeviceStatus, NetworkDevice, RxFrame, TxStatus, MAX_FRAME};
use rv2k_api::syscall::Error;
pub const BASE: usize = 0x50000000;
pub const RX: usize = 0x100;
pub const TX: usize = 0x800;
pub trait VirtualIo {
    fn read(&mut self, offset: usize) -> u32;
    fn write(&mut self, offset: usize, value: u32);
    fn read_byte(&mut self, offset: usize) -> u8;
    fn write_byte(&mut self, offset: usize, value: u8);
}
pub struct Mmio {
    base: usize,
}
impl VirtualIo for Mmio {
    fn read(&mut self, offset: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.base + offset) as *const u32) }
    }
    fn write(&mut self, offset: usize, value: u32) {
        unsafe {
            core::ptr::write_volatile((self.base + offset) as *mut u32, value);
        }
    }
    fn read_byte(&mut self, offset: usize) -> u8 {
        unsafe { core::ptr::read_volatile((self.base + offset) as *const u8) }
    }
    fn write_byte(&mut self, offset: usize, value: u8) {
        unsafe {
            core::ptr::write_volatile((self.base + offset) as *mut u8, value);
        }
    }
}
pub struct VirtualNic<I> {
    io: I,
    active: Option<RxFrame>,
    tx_len: usize,
    written: usize,
    building: bool,
}
impl<I: VirtualIo> VirtualNic<I> {
    pub const fn from_io(io: I) -> Self {
        Self {
            io,
            active: None,
            tx_len: 0,
            written: 0,
            building: false,
        }
    }
    pub fn finished(&mut self) -> bool {
        self.io.read(20) != 0
    }
    pub fn into_io(self) -> I {
        self.io
    }
}
impl VirtualNic<Mmio> {
    /// # Safety
    /// The virtual device window BASE..BASE+4096 must be mapped, have the documented
    /// register semantics, and be exclusively owned by this network task.
    pub const unsafe fn mapped() -> Self {
        Self::from_io(Mmio { base: BASE })
    }
}
impl<I: VirtualIo> NetworkDevice for VirtualNic<I> {
    #[inline(always)]
    fn poll(&mut self, _: u32) -> Result<DeviceStatus, Error> {
        let status = self.io.read(0);
        let tx = if status & 16 != 0 {
            TxStatus::Failed
        } else if status & 4 != 0 || self.building {
            TxStatus::Busy
        } else if status & 8 != 0 {
            TxStatus::Complete
        } else {
            TxStatus::Idle
        };
        if status & 24 != 0 {
            self.io.write(16, 4);
        }
        Ok(DeviceStatus {
            link_up: status & 1 != 0,
            rx_pending: status & 2 != 0,
            tx,
        })
    }
    #[inline(always)]
    fn rx_begin(&mut self) -> Result<Option<RxFrame>, Error> {
        if self.active.is_some() {
            return Err(Error::Busy);
        }
        if self.io.read(0) & 2 == 0 {
            return Ok(None);
        }
        let len = self.io.read(4);
        if len > MAX_FRAME as u32 {
            self.io.write(16, 1);
            return Err(Error::MessageSize);
        }
        let f = RxFrame {
            token: self.io.read(8),
            len: len as u16,
        };
        self.active = Some(f);
        Ok(Some(f))
    }
    #[cfg_attr(not(feature = "compact"), inline(always))]
    #[cfg_attr(feature = "compact", inline(never))]
    fn rx_read_at(&mut self, f: RxFrame, offset: usize, out: &mut [u8]) -> Result<(), Error> {
        if self.active != Some(f)
            || offset
                .checked_add(out.len())
                .is_none_or(|end| end > f.len as usize)
        {
            return Err(Error::Invalid);
        }
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = self.io.read_byte(RX + offset + i);
        }
        Ok(())
    }
    #[inline(always)]
    fn rx_release(&mut self, f: RxFrame) -> Result<(), Error> {
        if self.active != Some(f) {
            return Err(Error::Invalid);
        }
        self.io.write(16, 1);
        self.active = None;
        Ok(())
    }
    #[inline(always)]
    fn tx_begin(&mut self, len: usize) -> Result<(), Error> {
        if self.building || self.io.read(0) & 4 != 0 {
            return Err(Error::Again);
        }
        if !(14..=MAX_FRAME).contains(&len) {
            return Err(Error::MessageSize);
        }
        self.io.write(12, len as u32);
        self.tx_len = len;
        self.written = 0;
        self.building = true;
        Ok(())
    }
    #[cfg_attr(not(feature = "compact"), inline(always))]
    #[cfg_attr(feature = "compact", inline(never))]
    fn tx_write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if !self.building {
            return Err(Error::Invalid);
        }
        if bytes.len() > self.tx_len - self.written {
            return Err(Error::MessageSize);
        }
        for byte in bytes {
            self.io.write_byte(TX + self.written, *byte);
            self.written += 1;
        }
        Ok(())
    }
    #[inline(always)]
    fn tx_commit(&mut self, _: u32) -> Result<(), Error> {
        if !self.building || self.written != self.tx_len {
            return Err(Error::Invalid);
        }
        self.io.write(16, 2);
        self.building = false;
        Ok(())
    }
    #[inline(always)]
    fn tx_abort(&mut self) -> Result<(), Error> {
        self.io.write(16, 3);
        self.building = false;
        Ok(())
    }
}
