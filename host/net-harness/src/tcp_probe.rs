//! Independent packet peer for the real guest. No rv2k-net code is used here.
use std::collections::VecDeque;
const HOST: [u8; 4] = [192, 168, 123, 1];
const GUEST: [u8; 4] = [192, 168, 123, 50];
const HOST_MAC: [u8; 6] = [2, 0xaa, 0xbb, 0xcc, 0xdd, 0xee];
const GUEST_MAC: [u8; 6] = [2, 0, 0, 0, 0, 1];
const FIN: u8 = 1;
const SYN: u8 = 2;
const ACK: u8 = 16;
fn word(b: &[u8]) -> u16 {
    u16::from_be_bytes(b[..2].try_into().unwrap())
}
fn long(b: &[u8]) -> u32 {
    u32::from_be_bytes(b[..4].try_into().unwrap())
}
fn checksum(b: &[u8]) -> u16 {
    let mut sum = 0u32;
    for c in b.chunks(2) {
        sum += ((c[0] as u32) << 8) | c.get(1).copied().unwrap_or(0) as u32;
    }
    while sum >> 16 != 0 {
        sum = (sum & 65535) + (sum >> 16);
    }
    !(sum as u16)
}
fn frame(port: u16, seq: u32, ack: u32, flags: u8, body: &[u8]) -> Vec<u8> {
    let options = if flags & SYN != 0 {
        &[2, 4, 0, 24, 30, 4, 1, 2, 1, 0, 0, 0][..]
    } else {
        &[]
    };
    let tcp_len = 20 + options.len() + body.len();
    let mut f = vec![0; (54 + options.len() + body.len()).max(60)];
    f[..6].copy_from_slice(&GUEST_MAC);
    f[6..12].copy_from_slice(&HOST_MAC);
    f[12..16].copy_from_slice(&[8, 0, 0x45, 0]);
    f[16..18].copy_from_slice(&((20 + tcp_len) as u16).to_be_bytes());
    f[22] = 64;
    f[23] = 6;
    f[26..30].copy_from_slice(&HOST);
    f[30..34].copy_from_slice(&GUEST);
    let sum = checksum(&f[14..34]);
    f[24..26].copy_from_slice(&sum.to_be_bytes());
    f[34..36].copy_from_slice(&port.to_be_bytes());
    f[36..38].copy_from_slice(&1234u16.to_be_bytes());
    f[38..42].copy_from_slice(&seq.to_be_bytes());
    f[42..46].copy_from_slice(&ack.to_be_bytes());
    f[46] = ((20 + options.len()) / 4) as u8 * 16;
    f[47] = flags;
    f[48..50].copy_from_slice(&128u16.to_be_bytes());
    f[54..54 + options.len()].copy_from_slice(options);
    f[54 + options.len()..34 + tcp_len].copy_from_slice(body);
    let mut pseudo = HOST.to_vec();
    pseudo.extend_from_slice(&GUEST);
    pseudo.extend_from_slice(&[0, 6]);
    pseudo.extend_from_slice(&(tcp_len as u16).to_be_bytes());
    pseudo.extend_from_slice(&f[34..34 + tcp_len]);
    let sum = checksum(&pseudo);
    f[50..52].copy_from_slice(&sum.to_be_bytes());
    f
}
pub struct Probe {
    pub queue: VecDeque<Vec<u8>>,
    pub inputs: Vec<Vec<u8>>,
    pub advance_ticks: u32,
    pub advanced_ticks: u32,
    pub connections: u32,
    pub echoed_bytes: usize,
    pub syn_loss: bool,
    pub data_loss: bool,
    pub fin_loss: bool,
    waiting: bool,
    started: bool,
    done: bool,
    first_syn: Option<u32>,
    server_start: u32,
    server_next: u32,
    client_next: u32,
    expected: Vec<u8>,
    received: Vec<u8>,
    fin_sent: bool,
    fin_seen: bool,
    dropped_syn: bool,
    dropped_data: bool,
    dropped_fin: bool,
}
impl Probe {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            inputs: Vec::new(),
            advance_ticks: 0,
            advanced_ticks: 0,
            connections: 0,
            echoed_bytes: 0,
            syn_loss: false,
            data_loss: false,
            fin_loss: false,
            waiting: true,
            started: false,
            done: false,
            first_syn: None,
            server_start: 0,
            server_next: 0,
            client_next: 0,
            expected: Vec::new(),
            received: Vec::new(),
            fin_sent: false,
            fin_seen: false,
            dropped_syn: false,
            dropped_data: false,
            dropped_fin: false,
        }
    }
    pub fn done(&self) -> bool {
        self.done
    }
    pub fn needs_ready(&self) -> bool {
        self.waiting
    }
    fn port(&self) -> u16 {
        50000 + self.connections as u16
    }
    fn enqueue(&mut self, seq: u32, ack: u32, flags: u8, body: &[u8]) {
        let f = frame(self.port(), seq, ack, flags, body);
        self.inputs.push(f.clone());
        self.queue.push_back(f);
    }
    pub fn ready(&mut self, listens: u32, irqs: u32) {
        if !self.waiting || irqs == 0 || listens <= self.connections {
            return;
        }
        if self.connections == 2 {
            self.done = true;
            self.waiting = false;
            return;
        }
        if !self.started {
            self.advance_ticks = 6000;
            self.started = true;
        }
        self.waiting = false;
        self.first_syn = None;
        self.fin_sent = false;
        self.fin_seen = false;
        self.expected = (0..if self.connections == 0 { 65 } else { 128 })
            .map(|i| (i * 17 + 3) as u8)
            .collect();
        self.received.clear();
        self.client_next = (u32::MAX - 17).wrapping_add(self.connections * 1000);
        self.enqueue(self.client_next, 0, SYN, &[]);
        self.client_next = self.client_next.wrapping_add(1);
    }
    pub fn receive(&mut self, f: &[u8]) -> Result<(), String> {
        if f.len() < 54
            || f[..6] != HOST_MAC
            || f[6..12] != GUEST_MAC
            || f[12..14] != [8, 0]
            || f[23] != 6
        {
            return Err("unexpected non-TCP guest frame".into());
        }
        let ihl = (f[14] & 15) as usize * 4;
        let total = word(&f[16..18]) as usize;
        if ihl < 20
            || total < ihl + 20
            || 14 + total > f.len()
            || checksum(&f[14..14 + ihl]) != 0
            || f[26..30] != GUEST
            || f[30..34] != HOST
        {
            return Err("invalid guest IPv4 header".into());
        }
        let start = 14 + ihl;
        let tcp = &f[start..14 + total];
        let size = (tcp[12] >> 4) as usize * 4;
        if size < 20 || size > tcp.len() || word(tcp) != 1234 || word(&tcp[2..]) != self.port() {
            return Err("invalid guest TCP header or tuple".into());
        }
        let mut pseudo = GUEST.to_vec();
        pseudo.extend_from_slice(&HOST);
        pseudo.extend_from_slice(&[0, 6]);
        pseudo.extend_from_slice(&(tcp.len() as u16).to_be_bytes());
        pseudo.extend_from_slice(tcp);
        if checksum(&pseudo) != 0 {
            return Err("invalid guest TCP checksum".into());
        }
        let seq = long(&tcp[4..]);
        let flags = tcp[13];
        let payload = &tcp[size..];
        if flags & 4 != 0 {
            return Err("guest reset test connection".into());
        }
        if flags & SYN != 0 {
            if flags & (SYN | ACK) != (SYN | ACK)
                || long(&tcp[8..]) != self.client_next
                || tcp[20..] != [2, 4, 0, 128]
            {
                return Err("invalid SYN ACK".into());
            }
            if let Some(first) = self.first_syn {
                if first != seq {
                    return Err("SYN retransmission changed sequence".into());
                }
                self.syn_loss = true;
            } else {
                self.first_syn = Some(seq);
            }
            if self.connections == 0 && !self.dropped_syn {
                self.dropped_syn = true;
                self.advance_ticks = 100;
                return Ok(());
            }
            self.server_start = seq.wrapping_add(1);
            self.server_next = self.server_start;
            self.enqueue(self.client_next, self.server_next, ACK, &[]);
            let bytes = self.expected.clone();
            if self.connections == 0 {
                self.enqueue(
                    self.client_next.wrapping_add(19),
                    self.server_next,
                    ACK,
                    &bytes[19..],
                ); // out of order
                self.enqueue(self.client_next, self.server_next, ACK, &bytes[..19]);
                self.enqueue(self.client_next, self.server_next, ACK, &bytes[..19]); // duplicate
                self.enqueue(
                    self.client_next.wrapping_add(19),
                    self.server_next,
                    ACK,
                    &bytes[19..],
                );
            } else {
                self.enqueue(self.client_next, self.server_next, ACK, &bytes);
            }
            self.client_next = self.client_next.wrapping_add(bytes.len() as u32);
            return Ok(());
        }
        if !payload.is_empty() {
            let offset = seq.wrapping_sub(self.server_start) as usize;
            if offset + payload.len() > self.expected.len()
                || self.expected[offset..offset + payload.len()] != *payload
            {
                return Err("echo payload or retransmission differs".into());
            }
            if seq == self.server_next {
                self.received.extend_from_slice(payload);
                self.server_next = self.server_next.wrapping_add(payload.len() as u32);
            } else if offset + payload.len() > self.received.len() {
                return Err("guest sent out of order payload".into());
            } else if self.connections == 0 && self.dropped_data {
                self.data_loss = true;
            }
            if self.connections == 0 && !self.dropped_data {
                self.dropped_data = true;
                self.advance_ticks = 300;
                return Ok(());
            }
            self.enqueue(self.client_next, self.server_next, ACK, &[]);
            if self.received == self.expected && !self.fin_sent {
                self.enqueue(self.client_next, self.server_next, ACK | FIN, &[]);
                self.client_next = self.client_next.wrapping_add(1);
                self.fin_sent = true;
            }
        }
        if flags & FIN != 0 {
            if !self.fin_sent || self.received != self.expected {
                return Err("guest FIN before complete echo".into());
            }
            if !self.fin_seen {
                if seq != self.server_next {
                    return Err("FIN sequence mismatch".into());
                }
                self.server_next = self.server_next.wrapping_add(1);
                self.fin_seen = true;
            } else if seq.wrapping_add(1) != self.server_next {
                return Err("FIN retransmission changed sequence".into());
            }
            if self.connections == 0 && !self.dropped_fin {
                self.dropped_fin = true;
                self.advance_ticks = 100;
                return Ok(());
            }
            if self.connections == 0 {
                self.fin_loss = true;
            }
            self.enqueue(self.client_next, self.server_next, ACK, &[]);
            self.echoed_bytes += self.received.len();
            self.connections += 1;
            self.waiting = true;
        }
        Ok(())
    }
    pub fn take_advance(&mut self) -> u32 {
        let ticks = core::mem::take(&mut self.advance_ticks);
        self.advanced_ticks += ticks;
        ticks
    }
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({"done":self.done,"connections":self.connections,"echoed_bytes":self.echoed_bytes,
            "syn_loss_recovered":self.syn_loss,"data_ack_loss_recovered":self.data_loss,"fin_ack_loss_recovered":self.fin_loss,"advanced_ticks":self.advanced_ticks})
    }
}
