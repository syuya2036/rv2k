//! Driver integration probe. Renvo has GPIO/UART but no ENC on these SPI pins.
use core::{
    mem::MaybeUninit,
    ptr::{addr_of_mut, write_volatile},
};
use rv2k_api::syscall::Error;
#[cfg(not(feature = "net"))]
use rv2k_drivers::network::NetworkDevice;
use rv2k_drivers::{
    board::{self, Ch32Pins, Ch32Tx, Pad},
    enc28j60::Enc28j60,
    network::MAX_FRAME,
    spi::SoftwareDevice,
    uart::Console,
};
use rv2k_kernel::{memory::UserMemory, services::ConsoleWrite};

// init runs once with interrupts masked, before either task can use a device.
// Device state is ordinary RAM; only MMIO accesses and observer counters are volatile.
static mut NIC: MaybeUninit<Enc28j60<SoftwareDevice<Ch32Pins>>> = MaybeUninit::uninit();
static mut UART: MaybeUninit<UartService> = MaybeUninit::uninit();
#[no_mangle]
static mut DRIVER_STATUS: u32 = 0;
pub struct UartService(Console<Ch32Tx>);
impl ConsoleWrite for UartService {
    fn write_user(
        &mut self,
        memory: &mut impl UserMemory,
        address: usize,
        len: usize,
    ) -> Result<usize, Error> {
        self.0.write_from(len, |out| memory.copy_in(address, out))
    }
}
#[cfg(feature = "net")]
pub unsafe fn network() -> &'static mut Enc28j60<SoftwareDevice<Ch32Pins>> {
    (&mut *addr_of_mut!(NIC)).assume_init_mut()
}
pub unsafe fn console() -> &'static mut UartService {
    (&mut *addr_of_mut!(UART)).assume_init_mut()
}
pub unsafe fn init() {
    let (spi, uart) = board::j4m6();
    let spi = spi
        .into_device()
        .unwrap_or_else(|_| panic!("SPI selected during init"));
    (&mut *addr_of_mut!(UART)).write(UartService(uart));
    (&mut *addr_of_mut!(NIC)).write(Enc28j60::new(spi));
    assert_eq!(board::claim_gpio(Pad::Pd1), Err(Error::Busy));
    assert_eq!(board::claim_gpio(Pad::Pa1), Err(Error::Busy));
    assert_eq!(board::claim_gpio(Pad::Pc5), Err(Error::Invalid));
    let nic = (&mut *addr_of_mut!(NIC)).assume_init_mut();
    let status = match nic.reset([2, 0, 0, 0, 0, 1]) {
        Err(Error::Io) => 1, // expected on Renvo: no NIC, floating/zero MISO
        Ok(_) => {
            #[cfg(not(feature = "net"))]
            {
                // Keep all streaming driver methods in the RV32EC link. Actual RX/TX
                // execution requires an attached NIC and is a separate hardware gate.
                let _ = nic.poll(0);
                if let Ok(Some(frame)) = nic.rx_begin() {
                    let mut scratch = [0u8; 16];
                    let count = (frame.len as usize).min(scratch.len());
                    let _ = nic.rx_read_at(frame, 0, &mut scratch[..count]);
                    let _ = nic.rx_release(frame);
                }
                let mut header = [0u8; 14];
                header[..6].fill(0xff);
                header[6] = 2;
                header[11] = 1;
                header[12..].copy_from_slice(&[0x88, 0xb5]);
                if nic.tx_begin(header.len()).is_ok() {
                    let _ = nic.tx_write(&header);
                    let _ = nic.tx_commit(0);
                    let _ = nic.tx_abort();
                }
            }
            2
        }
        _ => 3,
    };
    assert!(core::mem::size_of_val(nic) < 64);
    assert_eq!(MAX_FRAME, 1514);
    write_volatile(addr_of_mut!(DRIVER_STATUS), status);
}
pub unsafe fn poll() {
    let _ = console().0.poll(4);
}
pub unsafe fn complete() {
    let uart = console();
    let _ = uart.0.write(b"SERVICES OK\n");
    for _ in 0..4096 {
        let _ = uart.0.poll(4);
        if uart.0.pending() == 0 {
            break;
        }
    }
}
