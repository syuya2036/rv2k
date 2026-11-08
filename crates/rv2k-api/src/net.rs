//! Version 1 network snapshot. Read-only, copied synchronously by syscall 14.
pub const VERSION: u16 = 1;
pub const LINK_UP: u8 = 1;
pub const PEER_PRESENT: u8 = 2;
pub const NETWORK_STOPPED: u8 = 4;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TcpState {
    Closed,
    Listen,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    Closing,
    CloseWait,
    LastAck,
    TimeWait,
}

/// Counters saturate at 65535; link/TCP state reflects the last network poll.
/// NETWORK_STOPPED marks an exited network task, even if its cached link is up.
/// IP octets are in network order; integer fields use little-endian wire order.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetInfo {
    pub version: u16,
    pub state: TcpState,
    pub flags: u8,
    pub local_ip: [u8; 4],
    pub peer_ip: [u8; 4],
    pub local_port: u16,
    pub peer_port: u16,
    pub received: u16,
    pub transmitted: u16,
    pub dropped: u16,
    pub bad_checksum: u16,
    pub fragments: u16,
    pub io_errors: u16,
    pub retransmissions: u16,
    pub resets: u16,
}
pub const BYTES: usize = 32;
const _: () = assert!(core::mem::size_of::<NetInfo>() == BYTES);
const _: () = assert!(core::mem::align_of::<NetInfo>() == 2);
impl NetInfo {
    pub fn with_bytes<T>(&self, visit: impl FnOnce(&[u8]) -> T) -> T {
        #[cfg(target_endian = "little")]
        {
            // repr(C) has no padding: 4 + 8 + 10*2 = 32 initialized bytes.
            visit(unsafe { core::slice::from_raw_parts((self as *const Self).cast(), BYTES) })
        }
        #[cfg(target_endian = "big")]
        {
            visit(&self.encode())
        }
    }
    pub fn encode(&self) -> [u8; BYTES] {
        let mut out = [0; BYTES];
        out[..2].copy_from_slice(&self.version.to_le_bytes());
        out[2] = self.state as u8;
        out[3] = self.flags;
        out[4..8].copy_from_slice(&self.local_ip);
        out[8..12].copy_from_slice(&self.peer_ip);
        for (index, value) in [
            self.local_port,
            self.peer_port,
            self.received,
            self.transmitted,
            self.dropped,
            self.bad_checksum,
            self.fragments,
            self.io_errors,
            self.retransmissions,
            self.resets,
        ]
        .into_iter()
        .enumerate()
        {
            out[12 + index * 2..14 + index * 2].copy_from_slice(&value.to_le_bytes());
        }
        out
    }
}

#[cfg(target_arch = "riscv32")]
pub fn info() -> Result<NetInfo, crate::syscall::Error> {
    use crate::syscall::{raw, Error, Number};
    let mut out = core::mem::MaybeUninit::<NetInfo>::uninit();
    unsafe {
        Error::decode(raw(
            Number::NetInfo as usize,
            [out.as_mut_ptr() as usize, BYTES, 0],
        ))?;
        Ok(out.assume_init())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versioned_wire_image_has_exact_offsets_and_no_padding() {
        let info = NetInfo {
            version: VERSION,
            state: TcpState::Established,
            flags: LINK_UP | PEER_PRESENT,
            local_ip: [192, 168, 123, 50],
            peer_ip: [192, 168, 123, 1],
            local_port: 23,
            peer_port: 50000,
            received: 1,
            transmitted: 2,
            dropped: 3,
            bad_checksum: 4,
            fragments: 5,
            io_errors: 6,
            retransmissions: 0x1234,
            resets: u16::MAX,
        };
        let expected = [
            1, 0, 3, 3, 192, 168, 123, 50, 192, 168, 123, 1, 23, 0, 80, 195, 1, 0, 2, 0, 3, 0, 4,
            0, 5, 0, 6, 0, 52, 18, 255, 255,
        ];
        assert_eq!(info.encode(), expected);
        info.with_bytes(|bytes| assert_eq!(bytes, expected));
        assert_eq!(TcpState::TimeWait as u8, 9);
    }
}
