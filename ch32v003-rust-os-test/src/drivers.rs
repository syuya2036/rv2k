//! Driver integration probe. Renvo has GPIO/UART but no ENC on these SPI pins.
use core::ptr::{addr_of_mut, write_volatile};
use rv2k_api::syscall::Error;
use rv2k_drivers::{
    board::{self, Ch32Pins, Ch32Tx, Pad},
    enc28j60::Enc28j60,
    network::{NetworkDevice, MAX_FRAME},
    spi::SoftwareSpi,
    uart::Console,
};
use rv2k_kernel::{memory::UserMemory, services::ConsoleWrite};

static mut NIC: Option<Enc28j60<SoftwareSpi<Ch32Pins>>> = None;
static mut UART: Option<UartService> = None;
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
pub unsafe fn console() -> &'static mut UartService {
    (&mut *addr_of_mut!(UART)).as_mut().unwrap()
}
pub unsafe fn init() {
    let (spi, uart) = board::j4m6();
    write_volatile(addr_of_mut!(UART), Some(UartService(uart)));
    write_volatile(addr_of_mut!(NIC), Some(Enc28j60::new(spi)));
    assert_eq!(board::claim_gpio(Pad::Pd1), Err(Error::Busy));
    assert_eq!(board::claim_gpio(Pad::Pa1), Err(Error::Busy));
    assert_eq!(board::claim_gpio(Pad::Pc5), Err(Error::Invalid));
    let nic = (&mut *addr_of_mut!(NIC)).as_mut().unwrap();
    let status = match nic.reset([2, 0, 0, 0, 0, 1]) {
        Err(Error::Io) => 1, // expected on Renvo: no NIC, floating/zero MISO
        Ok(_) => {
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
