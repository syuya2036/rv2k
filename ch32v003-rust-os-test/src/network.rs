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

// The public 64-bit uptime copy needs an aligned temporary. End that frame
// before entering the deeper protocol/driver chain.
#[inline(never)]
fn sample_time() -> u32 {
    time::uptime().unwrap_or(0) as u32
}
#[inline(never)]
pub fn task() {
    loop {
        let now = sample_time();
        let bits = unsafe {
            #[cfg(feature = "preempt")]
            let _guard = rv2k_arch::preempt::disable();
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
                network.socket.take_events()
            }
            #[cfg(not(feature = "tcp"))]
            {
                0
            }
        };
        #[cfg(feature = "preempt")]
        rv2k_arch::preempt::checkpoint();
        if bits != 0 {
            let _ = event::signal(rv2k_api::TaskId::Application, bits);
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
    use core::ptr::addr_of;
    #[cfg(feature = "emulator-stop")]
    use core::ptr::{read_volatile, write_volatile};
    use rv2k_api::{socket, TaskId};
    use rv2k_kernel::{
        memory::{Region, UserMemory},
        socket::SocketService,
        Scheduler,
    };
    #[no_mangle]
    static mut APP_BUFFER: [u8; 32] = [0; 32];
    #[cfg(feature = "emulator-stop")]
    #[no_mangle]
    static mut APP_BYTES: u32 = 0;
    #[cfg(feature = "emulator-stop")]
    #[no_mangle]
    static mut APP_LISTENS: u32 = 0;
    #[cfg(feature = "emulator-stop")]
    #[no_mangle]
    static mut APP_MEM_CHECK: u32 = 0;
    #[cfg(feature = "emulator-stop")]
    #[no_mangle]
    static mut APP_GP_CHECK: u32 = 0;
    // Rust requires the configured gp. Poison it only inside this assembly
    // probe: two real timer IRQs and GetPid must preserve the task value while
    // kernel code uses its own linker-defined gp. Restore it before Rust resumes.
    #[cfg(feature = "emulator-stop")]
    core::arch::global_asm!(
        r#"
.section .text
.option push
.option norelax
.global app_gp_probe
app_gp_probe:
addi sp, sp, -16
sw gp, 0(sp)
li gp, 0x55
la t0, TIMER_IRQS
lw a3, 0(t0)
li t0, 0xe000f008
lw t1, 0(t0)
1: lw t2, 0(t0)
sub t2, t2, t1
li a4, 480000
bltu t2, a4, 1b
la t0, TIMER_IRQS
lw a4, 0(t0)
sub a4, a4, a3
li t0, 2
li a0, 0
bltu a4, t0, 2f
li a5, 2
ecall
li t0, 0x55
bne gp, t0, 3f
li t0, 1
beq a0, t0, 2f
3: li a0, 0
2: lw gp, 0(sp)
addi sp, sp, 16
ret
.option pop
"#
    );
    struct Service<'a>(&'a mut Network);
    impl SocketService for Service<'_> {
        fn info(&self) -> Result<rv2k_api::net::NetInfo, Error> {
            Ok(self.0.info())
        }
        fn listen(&mut self, port: u16) -> Result<usize, Error> {
            self.0.socket.listen(port)
        }
        fn recv(
            &mut self,
            m: &mut impl UserMemory,
            h: usize,
            a: usize,
            n: usize,
        ) -> Result<usize, Error> {
            self.0
                .socket
                .recv_with(h, n, |bytes| m.copy_out(a, bytes, 1))
        }
        fn send(
            &mut self,
            m: &mut impl UserMemory,
            h: usize,
            a: usize,
            n: usize,
        ) -> Result<usize, Error> {
            self.0.socket.send_with(h, n, |out| m.copy_in(a, out))
        }
        fn close(&mut self, h: usize) -> Result<(), Error> {
            self.0.socket.close(h)
        }
        fn readiness(&self) -> u32 {
            self.0.socket.readiness()
        }
    }
    pub fn application_region() -> Region {
        let start = addr_of!(APP_BUFFER) as usize;
        Region {
            start,
            end: start + 32,
        }
    }
    pub unsafe fn socket_service() -> impl SocketService {
        Service(&mut *addr_of_mut!(NETWORK))
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
        #[cfg(feature = "emulator-stop")]
        unsafe {
            extern "C" {
                fn app_gp_probe() -> u32;
            }
            assert_eq!(app_gp_probe(), 1);
            write_volatile(addr_of_mut!(APP_GP_CHECK), 1);
        }
        #[cfg(feature = "emulator-stop")]
        {
            let info = rv2k_api::mem::info().expect("memory snapshot");
            assert!(
                info.version == 1
                    && info.guard_failures == 0
                    && info.static_bytes + info.kernel_reserved <= 2048
                    && info.flash_bytes <= 16384
            );
            unsafe {
                write_volatile(addr_of_mut!(APP_MEM_CHECK), 1);
            }
        }
        #[cfg(feature = "netinfo-probe")]
        crate::netinfo_probe::boot();
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
            #[cfg(feature = "emulator-stop")]
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
            #[cfg(feature = "netinfo-probe")]
            crate::netinfo_probe::connected();
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
                        #[cfg(feature = "emulator-stop")]
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
pub use sockets::{application, application_region, exited, socket_service};
