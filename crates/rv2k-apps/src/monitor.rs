//! Bounded text views of public OS snapshots; no formatter machinery or heap.
use rv2k_api::{
    mem::MemInfo,
    net::{NetInfo, TcpState},
    TaskInfo, TaskState,
};
pub const CAPACITY: usize = 64;
pub struct Line {
    bytes: [u8; CAPACITY],
    used: u8,
    // Reused decimal scratch belongs to the bounded output object, avoiding
    // an extra 28 bytes in each interrupted task call chain.
    words: [u16; 4],
    digits: [u8; 20],
}
impl Default for Line {
    fn default() -> Self {
        Self::new()
    }
}
impl Line {
    pub const fn new() -> Self {
        Self {
            bytes: [0; CAPACITY],
            used: 0,
            words: [0; 4],
            digits: [0; 20],
        }
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.used as usize]
    }
    fn clear(&mut self) {
        self.used = 0;
    }
    fn push(&mut self, bytes: &[u8]) {
        append(&mut self.bytes, &mut self.used, bytes);
    }
    fn number(&mut self, value: u64) {
        // Divide four base-65536 limbs using the shared word arithmetic helper.
        self.words = [
            value as u16,
            (value >> 16) as u16,
            (value >> 32) as u16,
            (value >> 48) as u16,
        ];
        let mut first = self.digits.len();
        loop {
            let mut carry = 0u32;
            for word in self.words.iter_mut().rev() {
                let combined = (carry << 16) | *word as u32;
                let (quotient, remainder) =
                    rv2k_api::integer::div_rem(combined, core::num::NonZeroU32::new(10).unwrap());
                *word = quotient as u16;
                carry = remainder;
            }
            first -= 1;
            self.digits[first] = b'0' + carry as u8;
            // Inspect the stored limbs after division rather than keeping an
            // OR accumulator live across every helper call on RV32E.
            if self.words[0] | self.words[1] | self.words[2] | self.words[3] == 0 {
                break;
            }
        }
        append(&mut self.bytes, &mut self.used, &self.digits[first..]);
    }
    #[inline(never)]
    fn field(&mut self, name: &[u8], value: u64) {
        self.push(name);
        self.number(value);
    }
    /// Bytes below SP encode a little-endian u16 offset; 0x80|offset is u8,
    /// 0xa0|offset is i16. Patterns are private, fixed ASCII snapshot layouts.
    #[inline(never)]
    fn view(&mut self, pattern: &[u8], source: &[u8]) {
        let mut start = 0;
        for (index, &tag) in pattern.iter().enumerate() {
            if !(32..128).contains(&tag) {
                self.push(&pattern[start..index]);
                let offset = (tag & 31) as usize;
                let mut value = source[offset] as u16;
                if tag & 0xe0 != 0x80 {
                    value |= (source[offset + 1] as u16) << 8;
                    if tag & 0xe0 == 0xa0 {
                        if (value as i16) < 0 {
                            self.push(b"-");
                        }
                        value = (value as i16).unsigned_abs();
                    }
                }
                self.number(value as u64);
                start = index + 1;
            }
        }
        self.push(&pattern[start..]);
    }
    #[inline(never)]
    pub fn task(&mut self, info: &TaskInfo) {
        self.clear();
        info.with_bytes(|bytes| self.view(b"pid=\x81", bytes));
        self.push(b" state=");
        self.push(match info.state {
            TaskState::Ready => b"READY",
            TaskState::Running => b"RUNNING",
            TaskState::Blocked => b"BLOCKED",
            TaskState::Exited => b"EXITED",
        });
        info.with_bytes(|bytes| self.view(b" exit=\xa4", bytes));
        self.push(b"\r\n");
    }
    #[inline(never)]
    pub fn memory(&mut self, info: &MemInfo, part: u8) {
        self.clear();
        match part {
            0 => {
                self.field(
                    b"ram=",
                    info.static_bytes as u64 + info.kernel_reserved as u64,
                );
                info.with_bytes(|bytes| self.view(b"/\x12 flash=\x10/\x14", bytes));
            }
            1 => info.with_bytes(|bytes| self.view(b"static=\x02 kernel=\x04 task=\x06", bytes)),
            2 => info.with_bytes(|bytes| {
                self.view(b"stack0=\x08 stack1=\x0a kernel=\x0c guards=\x0e", bytes)
            }),
            _ => return,
        }
        self.push(b"\r\n");
    }
    #[inline(never)]
    pub fn uptime(&mut self, ticks: u64) {
        self.clear();
        self.field(b"ticks=", ticks);
        self.push(b" tick_ms=10\r\n");
    }
    #[inline(never)]
    pub fn network(&mut self, info: &NetInfo, part: u8) {
        self.clear();
        match part {
            0 => {
                self.push(b"net state=");
                self.push(match info.state {
                    TcpState::Closed => b"CLOSED",
                    TcpState::Listen => b"LISTEN",
                    TcpState::SynReceived => b"SYN_RECEIVED",
                    TcpState::Established => b"ESTABLISHED",
                    TcpState::FinWait1 => b"FIN_WAIT_1",
                    TcpState::FinWait2 => b"FIN_WAIT_2",
                    TcpState::Closing => b"CLOSING",
                    TcpState::CloseWait => b"CLOSE_WAIT",
                    TcpState::LastAck => b"LAST_ACK",
                    TcpState::TimeWait => b"TIME_WAIT",
                });
                self.field(b" flags=", info.flags as u64);
            }
            1 => info.with_bytes(|bytes| {
                self.view(
                    b"local=\x84.\x85.\x86.\x87:\x0c peer=\x88.\x89.\x8a.\x8b:\x0e",
                    bytes,
                )
            }),
            2 => info.with_bytes(|bytes| self.view(b"rx=\x10 tx=\x12 drop=\x14 csum=\x16", bytes)),
            3 => info
                .with_bytes(|bytes| self.view(b"frag=\x18 io=\x1a retrans=\x1c reset=\x1e", bytes)),
            _ => return,
        }
        self.push(b"\r\n");
    }
}

#[inline(never)]
fn append(buffer: &mut [u8; CAPACITY], used: &mut u8, bytes: &[u8]) {
    let start = *used as usize;
    assert!(start + bytes.len() <= CAPACITY);
    buffer[start..start + bytes.len()].copy_from_slice(bytes);
    *used += bytes.len() as u8;
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use rv2k_api::TaskId;
    use std::{format, string::ToString};
    #[test]
    fn decimal_boundaries_and_wide_uptime_match_standard_formatter() {
        let mut line = Line::new();
        for n in [
            0,
            1,
            9,
            10,
            99,
            100,
            255,
            256,
            65535,
            u32::MAX as u64,
            1u64 << 32,
            (1u64 << 63) - 1,
            1u64 << 63,
            u64::MAX,
        ] {
            line.clear();
            line.number(n);
            assert_eq!(line.bytes(), n.to_string().as_bytes());
            line.uptime(n);
            assert_eq!(line.bytes(), format!("ticks={n} tick_ms=10\r\n").as_bytes());
        }
        let mut n = 17u64;
        for _ in 0..1000 {
            n = n.wrapping_mul(6364136223846793005).wrapping_add(1);
            line.clear();
            line.number(n);
            assert_eq!(line.bytes(), n.to_string().as_bytes());
        }
    }
    #[test]
    fn task_states_and_signed_exit_codes_use_public_snapshot() {
        let mut line = Line::new();
        for (state, name) in [
            (TaskState::Ready, "READY"),
            (TaskState::Running, "RUNNING"),
            (TaskState::Blocked, "BLOCKED"),
            (TaskState::Exited, "EXITED"),
        ] {
            for code in [i16::MIN, -7, 0, i16::MAX] {
                line.task(&TaskInfo {
                    version: 1,
                    id: TaskId::Application,
                    state,
                    reserved: 0,
                    exit_code: code,
                });
                assert_eq!(
                    line.bytes(),
                    format!("pid=1 state={name} exit={code}\r\n").as_bytes()
                );
            }
        }
    }
    #[test]
    fn largest_memory_and_network_views_fit_and_preserve_values() {
        let mut line = Line::new();
        let mem = MemInfo {
            version: 1,
            static_bytes: 65535,
            kernel_reserved: 65535,
            task_reserved: 65535,
            used: [65535; 3],
            guard_failures: 65535,
            flash_bytes: 65535,
            ram_limit: 65535,
            flash_limit: 65535,
            reserved: 0,
        };
        for (part, expected) in [
            b"ram=131070/65535 flash=65535/65535\r\n".as_slice(),
            b"static=65535 kernel=65535 task=65535\r\n",
            b"stack0=65535 stack1=65535 kernel=65535 guards=65535\r\n",
        ]
        .into_iter()
        .enumerate()
        {
            line.memory(&mem, part as u8);
            assert_eq!(line.bytes(), expected);
        }
        line.memory(&mem, 3);
        assert!(line.bytes().is_empty());
        let mut info = NetInfo {
            version: 1,
            state: TcpState::Established,
            flags: 7,
            local_ip: [255; 4],
            peer_ip: [255; 4],
            local_port: 65535,
            peer_port: 65535,
            received: 65535,
            transmitted: 65535,
            dropped: 65535,
            bad_checksum: 65535,
            fragments: 65535,
            io_errors: 65535,
            retransmissions: 65535,
            resets: 65535,
        };
        for (part, expected) in [
            b"net state=ESTABLISHED flags=7\r\n".as_slice(),
            b"local=255.255.255.255:65535 peer=255.255.255.255:65535\r\n",
            b"rx=65535 tx=65535 drop=65535 csum=65535\r\n",
            b"frag=65535 io=65535 retrans=65535 reset=65535\r\n",
        ]
        .into_iter()
        .enumerate()
        {
            line.network(&info, part as u8);
            assert_eq!(line.bytes(), expected);
        }
        line.network(&info, 4);
        assert!(line.bytes().is_empty());
        info.local_ip = [0, 1, 10, 100];
        info.peer_ip = [192, 168, 123, 50];
        info.local_port = 23;
        info.peer_port = 50000;
        line.network(&info, 1);
        assert_eq!(
            line.bytes(),
            b"local=0.1.10.100:23 peer=192.168.123.50:50000\r\n"
        );
        for (state, name) in [
            (TcpState::Closed, "CLOSED"),
            (TcpState::Listen, "LISTEN"),
            (TcpState::SynReceived, "SYN_RECEIVED"),
            (TcpState::Established, "ESTABLISHED"),
            (TcpState::FinWait1, "FIN_WAIT_1"),
            (TcpState::FinWait2, "FIN_WAIT_2"),
            (TcpState::Closing, "CLOSING"),
            (TcpState::CloseWait, "CLOSE_WAIT"),
            (TcpState::LastAck, "LAST_ACK"),
            (TcpState::TimeWait, "TIME_WAIT"),
        ] {
            info.state = state;
            line.network(&info, 0);
            assert_eq!(
                line.bytes(),
                format!("net state={name} flags=7\r\n").as_bytes()
            );
        }
    }
}
