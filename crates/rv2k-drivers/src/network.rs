use rv2k_api::syscall::Error;

pub const MAX_FRAME: usize = 1514; // Ethernet header + MTU, excludes FCS
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxFrame {
    pub token: u32,
    pub len: u16,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxStatus {
    Idle,
    Busy,
    Complete,
    Failed,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceStatus {
    pub link_up: bool,
    pub rx_pending: bool,
    pub tx: TxStatus,
}

/// Frames reside on the device. Borrowed operations must finish without yield;
/// the network task exclusively owns the device, including SPI transactions.
pub trait NetworkDevice {
    fn poll(&mut self, now: u32) -> Result<DeviceStatus, Error>;
    fn rx_begin(&mut self) -> Result<Option<RxFrame>, Error>;
    fn rx_read_at(&mut self, frame: RxFrame, offset: usize, out: &mut [u8]) -> Result<(), Error>;
    fn rx_release(&mut self, frame: RxFrame) -> Result<(), Error>;
    fn tx_begin(&mut self, len: usize) -> Result<(), Error>;
    fn tx_write(&mut self, bytes: &[u8]) -> Result<(), Error>;
    fn tx_commit(&mut self, now: u32) -> Result<(), Error>;
    fn tx_abort(&mut self) -> Result<(), Error>;
}
