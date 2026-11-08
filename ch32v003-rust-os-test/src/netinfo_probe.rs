//! Exercise the public copy ABI; observation buffers are not passed to syscalls.
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
use rv2k_api::{
    net::{self, NetInfo, TcpState},
    syscall::{self, Error, Number},
};
const EMPTY: NetInfo = NetInfo {
    version: 0,
    state: TcpState::Closed,
    flags: 0,
    local_ip: [0; 4],
    peer_ip: [0; 4],
    local_port: 0,
    peer_port: 0,
    received: 0,
    transmitted: 0,
    dropped: 0,
    bad_checksum: 0,
    fragments: 0,
    io_errors: 0,
    retransmissions: 0,
    resets: 0,
};
#[no_mangle]
static mut APP_NET_CHECK: u32 = 0;
#[no_mangle]
static mut APP_NET_RECORDS: u32 = 0;
#[no_mangle]
static mut APP_NET_SNAPSHOTS: [NetInfo; 3] = [EMPTY; 3];
fn record(info: &NetInfo, bits: u32) {
    unsafe {
        let count = read_volatile(addr_of!(APP_NET_RECORDS));
        if count < 3 {
            write_volatile(
                addr_of_mut!(APP_NET_SNAPSHOTS)
                    .cast::<NetInfo>()
                    .add(count as usize),
                *info,
            );
            write_volatile(addr_of_mut!(APP_NET_RECORDS), count + 1);
        }
        write_volatile(
            addr_of_mut!(APP_NET_CHECK),
            read_volatile(addr_of!(APP_NET_CHECK)) | bits,
        );
    }
}
#[inline(never)]
pub fn boot() {
    let info = net::info().expect("initial network snapshot");
    assert_eq!(info.version, net::VERSION);
    assert_eq!(info.state, TcpState::Closed);
    assert_eq!(info.local_ip, [192, 168, 123, 50]);
    assert_eq!((info.local_port, info.peer_port), (0, 0));
    assert_eq!(info.peer_ip, [0; 4]);
    assert_eq!((info.received, info.transmitted), (0, 0));
    // Wrong sizes/reserved argument reject before touching memory; valid sizes
    // still reject unaligned, kernel-owned, flash, and overflowing destinations.
    static CASES: [([usize; 3], Error); 7] = [
        ([0, 31, 0], Error::Invalid),
        ([0, 33, 0], Error::Invalid),
        ([0, 32, 1], Error::Invalid),
        ([1, 32, 0], Error::Fault),
        ([0x20000700, 32, 0], Error::Fault),
        ([0, 32, 0], Error::Fault),
        ([usize::MAX - 1, 32, 0], Error::Fault),
    ];
    for &(args, error) in &CASES {
        let result = unsafe { syscall::raw(Number::NetInfo as usize, args) };
        assert_eq!(result, error.code());
    }
    record(&info, 3);
}
#[inline(never)]
pub fn connected() {
    net::with_info(|info| {
        let before = *info;
        rv2k_api::task::yield_now();
        assert_eq!(*info, before);
        assert_eq!(info.version, net::VERSION);
        assert_eq!(info.state, TcpState::Established);
        assert_eq!(info.flags, net::LINK_UP | net::PEER_PRESENT);
        assert_eq!(info.local_ip, [192, 168, 123, 50]);
        assert_eq!(info.peer_ip, [192, 168, 123, 1]);
        assert_eq!(info.local_port, 1234);
        assert!(matches!(info.peer_port, 50000 | 50001));
        record(info, 4 | 32);
    })
    .expect("connected network snapshot");
}

#[inline(never)]
pub fn callbacks() {
    rv2k_api::mem::with_info(|info| {
        let before = *info;
        rv2k_api::task::yield_now();
        assert_eq!(*info, before);
        assert_eq!(info.version, 1);
        assert_eq!(info.guard_failures, 0);
    })
    .expect("memory callback snapshot");
    rv2k_api::task::with_info(rv2k_api::TaskId::Application, |info| {
        let before = *info;
        rv2k_api::task::yield_now();
        assert_eq!(*info, before);
        assert_eq!(info.id, rv2k_api::TaskId::Application);
        assert_eq!(info.state, rv2k_api::TaskState::Running);
    })
    .expect("task callback snapshot");
    unsafe {
        write_volatile(
            addr_of_mut!(APP_NET_CHECK),
            read_volatile(addr_of!(APP_NET_CHECK)) | 8 | 16,
        );
    }
}
