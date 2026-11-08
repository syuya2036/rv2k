//! The same streaming protocol core runs on the RV32EC network task.
use core::ptr::addr_of_mut;
#[cfg(not(feature = "drivers"))]
use core::ptr::write_volatile;
#[cfg(not(feature = "drivers"))]
use rv2k_api::TaskId;
use rv2k_api::{event, syscall::Error, time};
#[cfg(not(feature = "drivers"))]
use rv2k_drivers::virtual_nic::{Mmio, VirtualNic};
use rv2k_net::{Config, Network, Scratch};

static mut NETWORK: Network = Network::new(Config {
    mac: [2, 0, 0, 0, 0, 1],
    ip: [192, 168, 123, 50],
    mask: [255, 255, 255, 0],
});
static mut SCRATCH: Scratch = Scratch::new();
// This address is mapped only by net-harness; hardware uses the ENC28J60 driver.
#[cfg(not(feature = "drivers"))]
static mut DEVICE: VirtualNic<Mmio> = unsafe { VirtualNic::mapped() };
#[no_mangle]
static mut NET_COUNTERS: [u16; 6] = [0; 6];

pub fn task() {
    loop {
        let now = time::uptime().unwrap_or(0) as u32;
        unsafe {
            let network = &mut *addr_of_mut!(NETWORK);
            #[cfg(not(feature = "drivers"))]
            let device = &mut *addr_of_mut!(DEVICE);
            #[cfg(feature = "drivers")]
            let device = crate::drivers::network();
            #[cfg(not(feature = "drivers"))]
            if device.finished() {
                let s = &network.stats;
                write_volatile(
                    addr_of_mut!(NET_COUNTERS),
                    [
                        s.received,
                        s.transmitted,
                        s.dropped,
                        s.bad_checksum,
                        s.fragments,
                        s.io_errors,
                    ],
                );
                break;
            }
            if network
                .poll(device, &mut *addr_of_mut!(SCRATCH), now)
                .is_err()
            {
                panic!("network device error");
            }
        }
        match event::wait(event::DRIVER_POLL, 1) {
            Ok(_) | Err(Error::Timeout) => {}
            Err(_) => panic!("network wait failed"),
        }
    }
    #[cfg(not(feature = "drivers"))]
    let _ = event::signal(TaskId::Application, event::NET_CLOSED);
}
