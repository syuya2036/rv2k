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

#[cfg(feature = "tcp")]
static mut NETWORK: Network = Network::unconfigured();
#[cfg(not(feature = "tcp"))]
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
#[cfg(feature = "tcp")]
pub unsafe fn init() {
    (*addr_of_mut!(NETWORK)).configure(
        Config {
            mac: [2, 0, 0, 0, 0, 1],
            ip: [192, 168, 123, 50],
            mask: [255, 255, 255, 0],
        },
        0x10203040,
        6000,
    );
}

#[inline(never)]
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
            #[cfg(feature = "tcp")]
            {
                let bits = network.socket.take_events();
                if bits != 0 {
                    let _ = event::signal(rv2k_api::TaskId::Application, bits);
                }
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

#[cfg(feature = "tcp")]
mod sockets {
    use super::*;
    use core::ptr::{addr_of, read_volatile, write_volatile};
    use rv2k_api::{socket, TaskId};
    use rv2k_kernel::{
        ipc::Mailbox,
        memory::{Region, UserMemory},
        socket::SocketService,
        syscall::Outcome,
        Scheduler,
    };
    use rv2k_net::tcp::socket::Socket;
    #[no_mangle]
    static mut APP_BUFFER: [u8; 32] = [0; 32];
    #[no_mangle]
    static mut APP_BYTES: u32 = 0;
    #[no_mangle]
    static mut APP_LISTENS: u32 = 0;
    struct Service<'a>(&'a mut Socket);
    impl SocketService for Service<'_> {
        fn listen(&mut self, port: u16) -> Result<usize, Error> {
            self.0.listen(port)
        }
        fn recv(
            &mut self,
            m: &mut impl UserMemory,
            h: usize,
            a: usize,
            n: usize,
        ) -> Result<usize, Error> {
            self.0.recv_with(h, n, |bytes| m.copy_out(a, bytes, 1))
        }
        fn send(
            &mut self,
            m: &mut impl UserMemory,
            h: usize,
            a: usize,
            n: usize,
        ) -> Result<usize, Error> {
            self.0.send_with(h, n, |out| m.copy_in(a, out))
        }
        fn close(&mut self, h: usize) -> Result<(), Error> {
            self.0.close(h)
        }
        fn readiness(&self) -> u32 {
            self.0.readiness()
        }
    }
    pub fn application_region() -> Region {
        let start = addr_of!(APP_BUFFER) as usize;
        Region {
            start,
            end: start + 32,
        }
    }
    pub unsafe fn socket_dispatch(
        s: &mut Scheduler,
        m: &Mailbox,
        u: &mut impl UserMemory,
        n: usize,
        a: [usize; 3],
    ) -> Option<Outcome> {
        rv2k_kernel::socket::dispatch(
            s,
            m,
            u,
            n,
            a,
            &mut Service(&mut (*addr_of_mut!(NETWORK)).socket),
        )
    }
    pub unsafe fn exited(s: &mut Scheduler, id: TaskId) {
        let socket = &mut (*addr_of_mut!(NETWORK)).socket;
        if id == TaskId::Network {
            socket.network_stopped();
            let _ = s.signal(TaskId::Application, event::NET_CLOSED);
        } else {
            socket.application_stopped();
            let _ = s.signal(TaskId::Network, event::DRIVER_POLL);
        }
    }
    #[inline(never)]
    pub fn application() {
        loop {
            let handle = match socket::listen(1234) {
                Ok(h) => h,
                Err(Error::Closed) => return,
                Err(Error::Busy) => {
                    if event::wait(event::NET_CLOSED, 500) == Err(Error::Closed) {
                        return;
                    }
                    continue;
                }
                Err(_) => panic!("listen failed"),
            };
            unsafe {
                write_volatile(
                    addr_of_mut!(APP_LISTENS),
                    read_volatile(addr_of!(APP_LISTENS)) + 1,
                );
            }
            match event::wait(event::NET_CONNECTED | event::NET_CLOSED, event::FOREVER) {
                Ok(bits) if bits & event::NET_CONNECTED != 0 => {}
                Ok(_) => continue,
                Err(_) => return,
            }
            'connection: loop {
                let buffer = unsafe { &mut *addr_of_mut!(APP_BUFFER) };
                match socket::recv(handle, buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        let mut offset = 0;
                        while offset < count {
                            match socket::send(handle, &buffer[offset..count]) {
                                Ok(n) => offset += n,
                                Err(Error::Again) => {
                                    if event::wait(event::NET_TX_READY | event::NET_CLOSED, 500)
                                        .is_err()
                                    {
                                        break 'connection;
                                    }
                                }
                                Err(_) => break 'connection,
                            }
                        }
                        unsafe {
                            write_volatile(
                                addr_of_mut!(APP_BYTES),
                                read_volatile(addr_of!(APP_BYTES)) + count as u32,
                            );
                        }
                    }
                    Err(Error::Again) => {
                        if event::wait(event::NET_RX | event::NET_CLOSED, 500).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = socket::close(handle);
        }
    }
}
#[cfg(feature = "tcp")]
pub use sockets::{application, application_region, exited, socket_dispatch};
