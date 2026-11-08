#![no_std]
pub mod checksum;
pub mod tcp;
#[cfg(feature = "tcp")]
mod wire;
use checksum::{checksum, Checksum};
use rv2k_api::syscall::Error;
use rv2k_drivers::network::{NetworkDevice, RxFrame};
#[cfg(test)]
extern crate std;

#[repr(C, align(4))]
#[derive(Clone, Copy)]
pub struct Config {
    pub ip: [u8; 4],
    pub mask: [u8; 4],
    pub mac: [u8; 6],
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C, align(4))]
struct Peer {
    ip: [u8; 4],
    mac: [u8; 6],
}
#[derive(Clone, Copy)]
struct Neighbor {
    peer: Peer,
    deadline: u32,
}
#[derive(Clone, Copy)]
struct Query {
    ip: [u8; 4],
    deadline: u32,
    attempts: u8,
}
#[derive(Default)]
pub struct Stats {
    pub received: u16,
    pub transmitted: u16,
    pub dropped: u16,
    pub bad_checksum: u16,
    pub fragments: u16,
    pub io_errors: u16,
}
/// Statically owned by the network task; never allocate full frames on a stack.
pub struct Scratch {
    pub header: [u8; 96],
    pub stream: [u8; 64],
    #[cfg(feature = "tcp")]
    reception: Reception,
    #[cfg(feature = "tcp")]
    output: tcp::Outbound,
}
#[cfg(feature = "tcp")]
struct Reception {
    valid: bool,
    is_tcp: bool,
    packet: tcp::Segment,
    frame: RxFrame,
    start: usize,
    len: usize,
}
impl Scratch {
    pub const fn new() -> Self {
        Self {
            header: [0; 96],
            stream: [0; 64],
            #[cfg(feature = "tcp")]
            reception: Reception {
                valid: false,
                is_tcp: false,
                packet: tcp::Segment::empty(),
                frame: RxFrame { token: 0, len: 0 },
                start: 0,
                len: 0,
            },
            #[cfg(feature = "tcp")]
            output: tcp::Outbound::empty(),
        }
    }
}
impl Default for Scratch {
    fn default() -> Self {
        Self::new()
    }
}
pub struct Network {
    config: Config,
    neighbor: Option<Neighbor>,
    reply: Option<Peer>,
    query: Option<Query>,
    id: u16,
    pub stats: Stats,
    #[cfg(feature = "tcp")]
    pub tcp: tcp::Tcp,
    #[cfg(feature = "tcp")]
    pub socket: tcp::socket::Socket,
    #[cfg(feature = "tcp")]
    link_up: bool,
}
fn due(now: u32, deadline: u32) -> bool {
    now.wrapping_sub(deadline) < 0x80000000
}
fn be16(bytes: &[u8]) -> u16 {
    u16::from_be_bytes([bytes[0], bytes[1]])
}
fn unicast_mac(mac: &[u8]) -> bool {
    mac.len() == 6 && mac[0] & 1 == 0 && mac.iter().any(|b| *b != 0)
}
impl Network {
    #[inline(always)]
    fn frame_checksum(
        d: &mut impl NetworkDevice,
        f: RxFrame,
        start: usize,
        len: usize,
        stream: &mut [u8; 64],
        sum: &mut Checksum,
    ) -> Result<bool, Error> {
        let mut offset = 0;
        while offset < len {
            let count = stream.len().min(len - offset);
            d.rx_read_at(f, start + offset, &mut stream[..count])?;
            sum.update(&stream[..count]);
            offset += count;
        }
        Ok(sum.finish() == 0)
    }
    #[inline(never)]
    fn ipv4_header(
        &mut self,
        h: &mut [u8; 96],
        mac: &[u8; 6],
        ip: &[u8; 4],
        total: u16,
        protocol: u8,
    ) {
        h[..6].copy_from_slice(mac);
        h[6..12].copy_from_slice(&self.config.mac);
        h[12..14].copy_from_slice(&[8, 0]);
        h[14..34].fill(0);
        h[14] = 0x45;
        h[16..18].copy_from_slice(&total.to_be_bytes());
        self.id = self.id.wrapping_add(1);
        h[18..20].copy_from_slice(&self.id.to_be_bytes());
        h[20] = 0x40;
        h[22] = 64;
        h[23] = protocol;
        h[26..30].copy_from_slice(&self.config.ip);
        h[30..34].copy_from_slice(ip);
        let sum = checksum(&h[14..34]);
        h[24..26].copy_from_slice(&sum.to_be_bytes());
    }
    #[cfg(feature = "tcp")]
    pub const fn unconfigured() -> Self {
        let mut n = Self::new(Config {
            mac: [0; 6],
            ip: [0; 4],
            mask: [0; 4],
        });
        n.tcp = tcp::Tcp::new(0, 0);
        n
    }
    #[cfg(feature = "tcp")]
    pub fn configure(&mut self, config: Config, seed: u32, quiet_until: u32) {
        self.config = config;
        self.tcp.configure_boot(seed, quiet_until);
    }
    /// Observe without consuming events, acknowledgements, or pending requests.
    #[cfg(feature = "tcp")]
    pub fn info(&self) -> rv2k_api::net::NetInfo {
        use rv2k_api::net::{NetInfo, LINK_UP, PEER_PRESENT, VERSION};
        let peer = self.tcp.peer();
        NetInfo {
            version: VERSION,
            state: self.tcp.state(),
            flags: (if self.link_up { LINK_UP } else { 0 })
                | (if peer.is_some() { PEER_PRESENT } else { 0 }),
            local_ip: self.config.ip,
            peer_ip: peer.map_or([0; 4], |p| p.ip),
            local_port: self.socket.port(),
            peer_port: peer.map_or(0, |p| p.port),
            received: self.stats.received,
            transmitted: self.stats.transmitted,
            dropped: self.stats.dropped,
            bad_checksum: self.stats.bad_checksum,
            fragments: self.stats.fragments,
            io_errors: self.stats.io_errors,
            retransmissions: self.tcp.retransmissions,
            resets: self.tcp.resets,
        }
    }
    pub const fn new(config: Config) -> Self {
        Self {
            config,
            neighbor: None,
            reply: None,
            query: None,
            id: 0,
            #[cfg(feature = "tcp")]
            tcp: tcp::Tcp::new(0x10203040, 6000), // one 60-second MSL after reboot
            #[cfg(feature = "tcp")]
            socket: tcp::socket::Socket::new(),
            #[cfg(feature = "tcp")]
            link_up: false,
            stats: Stats {
                received: 0,
                transmitted: 0,
                dropped: 0,
                bad_checksum: 0,
                fragments: 0,
                io_errors: 0,
            },
        }
    }
    fn local_peer(&self, ip: [u8; 4]) -> bool {
        let address = u32::from_ne_bytes(ip);
        let own = u32::from_ne_bytes(self.config.ip);
        let mask = u32::from_ne_bytes(self.config.mask);
        let host = address & !mask;
        address != own
            && ip[0] != 0
            && ip[0] != 127
            && ip[0] < 224
            && address & mask == own & mask
            && host != 0
            && host != !mask
    }
    pub fn neighbor(&self, ip: [u8; 4], now: u32) -> Option<[u8; 6]> {
        self.neighbor
            .filter(|n| n.peer.ip == ip && !due(now, n.deadline))
            .map(|n| n.peer.mac)
    }
    /// Schedule bounded ARP resolution. No application data is acknowledged here.
    pub fn resolve(&mut self, ip: [u8; 4], now: u32) -> Result<Option<[u8; 6]>, Error> {
        if !self.local_peer(ip) {
            return Err(Error::Invalid);
        }
        if let Some(mac) = self.neighbor(ip, now) {
            return Ok(Some(mac));
        }
        if self.query.is_none_or(|q| q.ip != ip) {
            self.query = Some(Query {
                ip,
                deadline: now,
                attempts: 0,
            });
        }
        Ok(None)
    }
    #[inline(never)]
    pub fn poll(
        &mut self,
        device: &mut impl NetworkDevice,
        scratch: &mut Scratch,
        now: u32,
    ) -> Result<(), Error> {
        let result = self.poll_inner(device, scratch, now);
        #[cfg(feature = "tcp")]
        let result = if result.is_ok() && self.link_up {
            self.tcp_transmit(device, scratch, now)
        } else {
            if result.is_ok() {
                self.tcp.tick(&mut self.socket, now);
            }
            result
        };
        if result.is_err() {
            self.stats.io_errors = self.stats.io_errors.saturating_add(1);
        }
        result
    }
    #[cfg_attr(feature = "tcp", inline(never))]
    fn poll_inner(
        &mut self,
        device: &mut impl NetworkDevice,
        scratch: &mut Scratch,
        now: u32,
    ) -> Result<(), Error> {
        let status = device.poll(now)?;
        #[cfg(feature = "tcp")]
        {
            self.link_up = status.link_up;
        }
        if !status.link_up {
            return Ok(());
        }
        if self.neighbor.is_some_and(|n| due(now, n.deadline)) {
            self.neighbor = None;
        }
        if let Some(peer) = self.reply {
            match self.send_arp(device, scratch, peer, true, now) {
                Ok(()) => self.reply = None,
                Err(Error::Again) => {}
                Err(e) => return Err(e),
            }
        } else if let Some(query) = self.query.filter(|q| due(now, q.deadline)) {
            if query.attempts == 3 {
                self.query = None;
                return Err(Error::Timeout);
            }
            match self.send_arp(
                device,
                scratch,
                Peer {
                    mac: [0; 6],
                    ip: query.ip,
                },
                false,
                now,
            ) {
                Ok(()) => {
                    self.query = Some(Query {
                        deadline: now.wrapping_add(100),
                        attempts: query.attempts + 1,
                        ..query
                    })
                }
                Err(Error::Again) => {}
                Err(e) => return Err(e),
            }
        }
        if let Some(frame) = device.rx_begin()? {
            self.stats.received = self.stats.received.saturating_add(1);
            #[cfg(feature = "tcp")]
            {
                scratch.reception.valid = false;
                scratch.reception.is_tcp = false;
            }
            let result = self.receive(device, scratch, frame, now);
            #[cfg(feature = "tcp")]
            let result = if result.is_ok() && scratch.reception.is_tcp {
                self.tcp_receive(device, scratch)
            } else {
                result
            };
            #[cfg(feature = "tcp")]
            let result = if result.is_ok() && scratch.reception.valid {
                let r = &scratch.reception;
                self.tcp
                    .receive(&mut self.socket, &r.packet, now, |skip, out| {
                        device.rx_read_at(frame, r.start + skip, out)
                    })
            } else {
                result
            };
            let release = device.rx_release(frame);
            result.and(release)?;
        }
        Ok(())
    }
    #[cfg_attr(feature = "tcp", inline(never))]
    fn drop_frame(&mut self) {
        self.stats.dropped = self.stats.dropped.saturating_add(1);
    }
    fn receive(
        &mut self,
        d: &mut impl NetworkDevice,
        s: &mut Scratch,
        f: RxFrame,
        now: u32,
    ) -> Result<(), Error> {
        if f.len < 14 {
            self.drop_frame();
            return Ok(());
        }
        d.rx_read_at(f, 0, &mut s.header[..14])?;
        if !unicast_mac(&s.header[6..12]) {
            self.drop_frame();
            return Ok(());
        }
        let own = s.header[..6] == self.config.mac;
        match be16(&s.header[12..14]) {
            0x0806 if own || s.header[..6] == [255; 6] => self.arp(d, s, f, now),
            0x0800 if own => self.ipv4(d, s, f, now),
            _ => {
                self.drop_frame();
                Ok(())
            }
        }
    }
    fn arp(
        &mut self,
        d: &mut impl NetworkDevice,
        s: &mut Scratch,
        f: RxFrame,
        now: u32,
    ) -> Result<(), Error> {
        if f.len < 42 {
            self.drop_frame();
            return Ok(());
        }
        d.rx_read_at(f, 14, &mut s.header[14..42])?;
        let h = &s.header;
        if h[14..20] != [0, 1, 8, 0, 6, 4] || h[20] != 0 || !(1..=2).contains(&h[21]) {
            self.drop_frame();
            return Ok(());
        }
        if h[22..28] != h[6..12] || h[38..42] != self.config.ip {
            self.drop_frame();
            return Ok(());
        }
        let peer = Peer {
            mac: h[22..28].try_into().unwrap(),
            ip: h[28..32].try_into().unwrap(),
        };
        let reply = h[21] == 1;
        if !reply && h[32..38] != self.config.mac {
            self.drop_frame();
            return Ok(());
        }
        if peer.ip != [0; 4] && !self.local_peer(peer.ip) {
            self.drop_frame();
            return Ok(());
        }
        if peer.ip != [0; 4] {
            self.neighbor = Some(Neighbor {
                peer,
                deadline: now.wrapping_add(3000),
            });
            if self.query.is_some_and(|q| q.ip == peer.ip) {
                self.query = None;
            }
        }
        if reply {
            match self.send_arp(d, s, peer, true, now) {
                Ok(()) => {}
                Err(Error::Again) => self.reply = Some(peer),
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    fn send_arp(
        &mut self,
        d: &mut impl NetworkDevice,
        s: &mut Scratch,
        peer: Peer,
        reply: bool,
        now: u32,
    ) -> Result<(), Error> {
        d.tx_begin(42)?;
        let h = &mut s.header;
        h[..42].fill(0);
        h[..6].copy_from_slice(if reply { &peer.mac } else { &[255; 6] });
        h[6..12].copy_from_slice(&self.config.mac);
        h[12..20].copy_from_slice(&[8, 6, 0, 1, 8, 0, 6, 4]);
        h[21] = if reply { 2 } else { 1 };
        h[22..28].copy_from_slice(&self.config.mac);
        h[28..32].copy_from_slice(&self.config.ip);
        h[32..38].copy_from_slice(&peer.mac);
        h[38..42].copy_from_slice(&peer.ip);
        let result = d.tx_write(&h[..42]).and_then(|_| d.tx_commit(now));
        if result.is_err() {
            let _ = d.tx_abort();
        } else {
            self.stats.transmitted = self.stats.transmitted.saturating_add(1);
        }
        result
    }
    #[cfg_attr(not(feature = "tcp"), inline(always))]
    #[cfg_attr(feature = "tcp", inline(never))]
    fn ipv4(
        &mut self,
        d: &mut impl NetworkDevice,
        s: &mut Scratch,
        f: RxFrame,
        now: u32,
    ) -> Result<(), Error> {
        if f.len < 34 {
            self.drop_frame();
            return Ok(());
        }
        d.rx_read_at(f, 14, &mut s.header[14..34])?;
        let ihl = (s.header[14] & 15) as usize * 4;
        let total = be16(&s.header[16..18]) as usize;
        if s.header[14] >> 4 != 4
            || !(20..=60).contains(&ihl)
            || total < ihl
            || total + 14 > f.len as usize
            || s.header[22] == 0
            || s.header[30..34] != self.config.ip
        {
            self.drop_frame();
            return Ok(());
        }
        if ihl > 20 {
            d.rx_read_at(f, 34, &mut s.header[34..14 + ihl])?;
        }
        if checksum(&s.header[14..14 + ihl]) != 0 {
            self.stats.bad_checksum = self.stats.bad_checksum.saturating_add(1);
            self.drop_frame();
            return Ok(());
        }
        // DF is allowed; reserved/MF bits and the fragment offset must be zero.
        if s.header[20] & 0xbf != 0 || s.header[21] != 0 {
            self.stats.fragments = self.stats.fragments.saturating_add(1);
            self.drop_frame();
            return Ok(());
        }
        let mut options = &s.header[34..14 + ihl];
        while !options.is_empty() {
            let kind = options[0];
            if kind == 0 {
                break;
            }
            if kind == 1 {
                options = &options[1..];
                continue;
            }
            if options.len() < 2
                || options[1] < 2
                || options[1] as usize > options.len()
                || kind == 131
                || kind == 137
            {
                self.drop_frame();
                return Ok(());
            }
            options = &options[options[1] as usize..];
        }
        let peer = Peer {
            mac: s.header[6..12].try_into().unwrap(),
            ip: s.header[26..30].try_into().unwrap(),
        };
        if !self.local_peer(peer.ip) {
            self.drop_frame();
            return Ok(());
        }
        #[cfg(feature = "tcp")]
        if s.header[23] == 6 {
            s.reception.packet.peer = tcp::Endpoint {
                mac: peer.mac,
                ip: peer.ip,
                port: 0,
            };
            s.reception.frame = f;
            s.reception.start = 14 + ihl;
            s.reception.len = total - ihl;
            s.reception.is_tcp = true;
            return Ok(());
        }
        if s.header[23] != 1 || total < ihl + 8 {
            self.drop_frame();
            return Ok(());
        }
        let start = 14 + ihl;
        let len = total - ihl;
        d.rx_read_at(f, start, &mut s.header[start..start + 8])?;
        if s.header[start] != 8 || s.header[start + 1] != 0 {
            self.drop_frame();
            return Ok(());
        }
        let mut sum = Checksum::new();
        if !Self::frame_checksum(d, f, start, len, &mut s.stream, &mut sum)? {
            self.stats.bad_checksum = self.stats.bad_checksum.saturating_add(1);
            self.drop_frame();
            return Ok(());
        }
        let old = be16(&s.header[start + 2..start + 4]) as u32 + 0x0800;
        let reply_checksum = ((old & 0xffff) + (old >> 16)) as u16;
        match d.tx_begin(34 + len) {
            Ok(()) => {}
            Err(Error::Again) => {
                self.drop_frame();
                return Ok(());
            }
            Err(e) => return Err(e),
        }
        s.header.copy_within(start..start + 8, 34);
        self.ipv4_header(&mut s.header, &peer.mac, &peer.ip, (20 + len) as u16, 1);
        s.header[34] = 0;
        s.header[36..38].copy_from_slice(&reply_checksum.to_be_bytes());
        let result = (|| {
            d.tx_write(&s.header[..42])?;
            let mut offset = 8;
            while offset < len {
                let count = s.stream.len().min(len - offset);
                d.rx_read_at(f, start + offset, &mut s.stream[..count])?;
                d.tx_write(&s.stream[..count])?;
                offset += count;
            }
            d.tx_commit(now)
        })();
        if result.is_err() {
            let _ = d.tx_abort();
        } else {
            self.stats.transmitted = self.stats.transmitted.saturating_add(1);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rv2k_drivers::network::{DeviceStatus, TxStatus};
    use std::{vec, vec::Vec};
    const HOST_MAC: [u8; 6] = [2, 0xaa, 0xbb, 0xcc, 0xdd, 0xee];
    const HOST_IP: [u8; 4] = [192, 168, 1, 1];
    fn config() -> Config {
        Config {
            mac: [2, 0, 0, 0, 0, 1],
            ip: [192, 168, 1, 50],
            mask: [255, 255, 255, 0],
        }
    }
    fn reference(bytes: &[u8]) -> u16 {
        let mut sum = 0u32;
        for part in bytes.chunks(2) {
            sum += ((part[0] as u32) << 8) | part.get(1).copied().unwrap_or(0) as u32;
        }
        while sum > 65535 {
            sum = (sum & 65535) + (sum >> 16);
        }
        !(sum as u16)
    }
    fn arp_request() -> Vec<u8> {
        let mut f = vec![0; 60];
        f[..6].fill(255);
        f[6..12].copy_from_slice(&HOST_MAC);
        f[12..22].copy_from_slice(&[8, 6, 0, 1, 8, 0, 6, 4, 0, 1]);
        f[22..28].copy_from_slice(&HOST_MAC);
        f[28..32].copy_from_slice(&HOST_IP);
        f[38..42].copy_from_slice(&config().ip);
        f
    }
    fn echo(len: usize, options: &[u8]) -> Vec<u8> {
        let ihl = 20 + options.len();
        assert_eq!(ihl % 4, 0);
        let total = ihl + 8 + len;
        let start = 14 + ihl;
        let mut f = vec![0; (14 + total).max(60)];
        f[..6].copy_from_slice(&config().mac);
        f[6..12].copy_from_slice(&HOST_MAC);
        f[12..14].copy_from_slice(&[8, 0]);
        f[14] = 0x40 | (ihl / 4) as u8;
        f[16..18].copy_from_slice(&(total as u16).to_be_bytes());
        f[22] = 64;
        f[23] = 1;
        f[26..30].copy_from_slice(&HOST_IP);
        f[30..34].copy_from_slice(&config().ip);
        f[34..34 + options.len()].copy_from_slice(options);
        let sum = reference(&f[14..14 + ihl]);
        f[24..26].copy_from_slice(&sum.to_be_bytes());
        f[start] = 8;
        f[start + 4..start + 8].copy_from_slice(&[0x12, 0x34, 0xff, 0xfe]);
        for i in 0..len {
            f[start + 8 + i] = i as u8;
        }
        let sum = reference(&f[start..14 + total]);
        f[start + 2..start + 4].copy_from_slice(&sum.to_be_bytes());
        f
    }
    struct Device {
        rx: Option<Vec<u8>>,
        active: bool,
        token: u32,
        tx: Vec<u8>,
        len: usize,
        out: Vec<Vec<u8>>,
        releases: usize,
        aborts: usize,
        busy: bool,
        fail_write: bool,
        link_up: bool,
    }
    impl Device {
        fn new(frame: Vec<u8>) -> Self {
            Self {
                rx: Some(frame),
                active: false,
                token: 1,
                tx: Vec::new(),
                len: 0,
                out: Vec::new(),
                releases: 0,
                aborts: 0,
                busy: false,
                fail_write: false,
                link_up: true,
            }
        }
    }
    impl NetworkDevice for Device {
        fn poll(&mut self, _: u32) -> Result<DeviceStatus, Error> {
            Ok(DeviceStatus {
                link_up: self.link_up,
                rx_pending: self.rx.is_some(),
                tx: if self.busy {
                    TxStatus::Busy
                } else {
                    TxStatus::Idle
                },
            })
        }
        fn rx_begin(&mut self) -> Result<Option<RxFrame>, Error> {
            assert!(!self.active);
            if let Some(f) = &self.rx {
                self.active = true;
                Ok(Some(RxFrame {
                    token: self.token,
                    len: f.len() as u16,
                }))
            } else {
                Ok(None)
            }
        }
        fn rx_read_at(&mut self, f: RxFrame, offset: usize, out: &mut [u8]) -> Result<(), Error> {
            assert!(self.active && f.token == self.token);
            out.copy_from_slice(&self.rx.as_ref().unwrap()[offset..offset + out.len()]);
            Ok(())
        }
        fn rx_release(&mut self, f: RxFrame) -> Result<(), Error> {
            assert!(self.active && f.token == self.token);
            self.active = false;
            self.rx = None;
            self.token += 1;
            self.releases += 1;
            Ok(())
        }
        fn tx_begin(&mut self, len: usize) -> Result<(), Error> {
            if self.busy {
                return Err(Error::Again);
            }
            assert_eq!(self.len, 0);
            self.len = len;
            self.tx.clear();
            Ok(())
        }
        fn tx_write(&mut self, b: &[u8]) -> Result<(), Error> {
            if self.fail_write {
                return Err(Error::Io);
            }
            assert!(self.tx.len() + b.len() <= self.len);
            self.tx.extend_from_slice(b);
            Ok(())
        }
        fn tx_commit(&mut self, _: u32) -> Result<(), Error> {
            assert_eq!(self.tx.len(), self.len);
            self.tx.resize(self.len.max(60), 0);
            self.out.push(self.tx.clone());
            self.len = 0;
            Ok(())
        }
        fn tx_abort(&mut self) -> Result<(), Error> {
            self.aborts += 1;
            self.len = 0;
            self.tx.clear();
            Ok(())
        }
    }
    #[test]
    fn arp_reply_cache_expiry_and_pending_tx_busy() {
        let mut n = Network::new(config());
        let mut s = Scratch::new();
        let mut d = Device::new(arp_request());
        d.busy = true;
        n.poll(&mut d, &mut s, u32::MAX - 10).unwrap();
        assert_eq!(d.releases, 1);
        assert!(d.out.is_empty());
        assert_eq!(n.neighbor(HOST_IP, 0), Some(HOST_MAC));
        d.busy = false;
        n.poll(&mut d, &mut s, 0).unwrap();
        let f = &d.out[0];
        assert_eq!(&f[..6], &HOST_MAC);
        assert_eq!(&f[20..22], &[0, 2]);
        assert_eq!(&f[22..28], &config().mac);
        assert_eq!(&f[38..42], &HOST_IP);
        assert_eq!(n.neighbor(HOST_IP, 2988), Some(HOST_MAC));
        assert_eq!(n.neighbor(HOST_IP, 2989), None);
    }
    #[test]
    fn icmp_stream_preserves_all_bytes_across_sizes_options_and_odd_lengths() {
        for len in [0, 1, 55, 56, 57, 63, 64, 65, 1472] {
            let mut n = Network::new(config());
            let mut s = Scratch::new();
            let input = echo(len, &[]);
            let mut d = Device::new(input.clone());
            n.poll(&mut d, &mut s, 0).unwrap();
            assert_eq!(d.releases, 1);
            assert_eq!(d.out.len(), 1);
            let f = &d.out[0];
            assert_eq!(&f[..6], &HOST_MAC);
            assert_eq!(&f[6..12], &config().mac);
            assert_eq!(reference(&f[14..34]), 0);
            assert_eq!(reference(&f[34..42 + len]), 0);
            assert_eq!(f[34], 0);
            assert_eq!(&f[38..42 + len], &input[38..42 + len]);
        }
        for options in [&[1, 1, 0, 0][..], &[0u8; 40][..]] {
            let input = echo(57, options);
            let mut n = Network::new(config());
            let mut d = Device::new(input.clone());
            n.poll(&mut d, &mut Scratch::new(), 0).unwrap();
            let f = &d.out[0];
            assert_eq!(f[14], 0x45);
            assert_eq!(&f[38..99], &input[38 + options.len()..99 + options.len()]);
            assert_eq!(reference(&f[34..99]), 0);
        }
    }
    #[test]
    fn malformed_frames_are_dropped_and_released_once() {
        let mut bad = Vec::new();
        bad.push(vec![0; 13]);
        for edit in 0..9 {
            let mut f = echo(3, &[]);
            match edit {
                0 => f[0] ^= 2,
                1 => f[14] = 0x44,
                2 => f[16..18].copy_from_slice(&1510u16.to_be_bytes()),
                3 => f[22] = 0,
                4 => f[24] ^= 1,
                5 => f[42] ^= 1,
                6 => f[34] = 3,
                7 => f[35] = 1,
                8 => f[20] = 0x20,
                _ => unreachable!(),
            }
            if [3, 8].contains(&edit) {
                f[24..26].fill(0);
                let sum = reference(&f[14..34]);
                f[24..26].copy_from_slice(&sum.to_be_bytes());
            }
            bad.push(f);
        }
        let mut bad_options = echo(3, &[7, 1, 0, 0]);
        bad_options[24..26].fill(0);
        let sum = reference(&bad_options[14..38]);
        bad_options[24..26].copy_from_slice(&sum.to_be_bytes());
        bad.push(bad_options);
        for f in bad {
            let mut n = Network::new(config());
            let mut d = Device::new(f);
            n.poll(&mut d, &mut Scratch::new(), 0).unwrap();
            assert!(d.out.is_empty());
            assert_eq!(d.releases, 1);
            assert_eq!(n.stats.dropped, 1);
        }
    }
    #[test]
    fn arp_resolution_retries_wrap_and_valid_reply_cancels_query() {
        let mut n = Network::new(config());
        let mut d = Device::new(Vec::new());
        d.rx = None;
        let mut s = Scratch::new();
        assert_eq!(n.resolve(HOST_IP, u32::MAX - 25), Ok(None));
        for now in [u32::MAX - 25, 73, 74, 174] {
            n.poll(&mut d, &mut s, now).unwrap();
        }
        assert_eq!(d.out.len(), 3);
        assert_eq!(n.poll(&mut d, &mut s, 274), Err(Error::Timeout));
        n.resolve(HOST_IP, 300).unwrap();
        n.poll(&mut d, &mut s, 300).unwrap();
        let mut reply = arp_request();
        reply[..6].copy_from_slice(&config().mac);
        reply[21] = 2;
        reply[32..38].copy_from_slice(&config().mac);
        d.rx = Some(reply);
        n.poll(&mut d, &mut s, 301).unwrap();
        assert_eq!(n.resolve(HOST_IP, 302), Ok(Some(HOST_MAC)));
        n.poll(&mut d, &mut s, 401).unwrap();
        assert_eq!(d.out.len(), 4);
    }
    #[test]
    fn tx_errors_abort_and_rx_is_released_even_on_error() {
        for f in [arp_request(), echo(100, &[])] {
            let mut n = Network::new(config());
            let mut d = Device::new(f);
            d.fail_write = true;
            assert_eq!(n.poll(&mut d, &mut Scratch::new(), 0), Err(Error::Io));
            assert_eq!(d.aborts, 1);
            assert_eq!(d.releases, 1);
            assert!(d.out.is_empty());
        }
        let mut n = Network::new(config());
        let mut d = Device::new(echo(56, &[]));
        d.busy = true;
        n.poll(&mut d, &mut Scratch::new(), 0).unwrap();
        assert_eq!(d.releases, 1);
        assert_eq!(n.stats.dropped, 1);
    }
    #[cfg(feature = "tcp")]
    mod wire_tests {
        use super::*;
        use crate::tcp::{State, Tcp, ACK, PSH, RST, SYN};
        fn frame(
            seq: u32,
            ack: u32,
            flags: u8,
            data: &[u8],
            options: &[u8],
            ip_options: &[u8],
        ) -> Vec<u8> {
            let ihl = 20 + ip_options.len();
            let thl = 20 + options.len();
            let total = ihl + thl + data.len();
            let mut f = vec![0; (14 + total).max(60)];
            f[..6].copy_from_slice(&config().mac);
            f[6..12].copy_from_slice(&HOST_MAC);
            f[12..14].copy_from_slice(&[8, 0]);
            f[14] = 0x40 | (ihl / 4) as u8;
            f[16..18].copy_from_slice(&(total as u16).to_be_bytes());
            f[22] = 64;
            f[23] = 6;
            f[26..30].copy_from_slice(&HOST_IP);
            f[30..34].copy_from_slice(&config().ip);
            f[34..34 + ip_options.len()].copy_from_slice(ip_options);
            let sum = reference(&f[14..14 + ihl]);
            f[24..26].copy_from_slice(&sum.to_be_bytes());
            let start = 14 + ihl;
            f[start..start + 2].copy_from_slice(&50000u16.to_be_bytes());
            f[start + 2..start + 4].copy_from_slice(&80u16.to_be_bytes());
            f[start + 4..start + 8].copy_from_slice(&seq.to_be_bytes());
            f[start + 8..start + 12].copy_from_slice(&ack.to_be_bytes());
            f[start + 12] = (thl / 4) as u8 * 16;
            f[start + 13] = flags;
            f[start + 14..start + 16].copy_from_slice(&128u16.to_be_bytes());
            f[start + 20..start + thl].copy_from_slice(options);
            f[start + thl..14 + total].copy_from_slice(data);
            fix_tcp(&mut f);
            f
        }
        fn fix_tcp(f: &mut [u8]) {
            let start = 14 + (f[14] & 15) as usize * 4;
            let end = 14 + be16(&f[16..18]) as usize;
            f[start + 16..start + 18].fill(0);
            let mut pseudo = f[26..34].to_vec();
            pseudo.extend_from_slice(&[0, 6]);
            pseudo.extend_from_slice(&((end - start) as u16).to_be_bytes());
            pseudo.extend_from_slice(&f[start..end]);
            let sum = reference(&pseudo);
            f[start + 16..start + 18].copy_from_slice(&sum.to_be_bytes());
        }
        fn validate(f: &[u8]) -> &[u8] {
            assert_eq!(&f[..6], &HOST_MAC);
            assert_eq!(&f[6..12], &config().mac);
            assert_eq!(f[14], 0x45);
            assert_eq!(f[23], 6);
            assert_eq!(reference(&f[14..34]), 0);
            let end = 14 + be16(&f[16..18]) as usize;
            let mut pseudo = f[26..34].to_vec();
            pseudo.extend_from_slice(&[0, 6]);
            pseudo.extend_from_slice(&((end - 34) as u16).to_be_bytes());
            pseudo.extend_from_slice(&f[34..end]);
            assert_eq!(reference(&pseudo), 0);
            assert_eq!(be16(&f[34..36]), 80);
            assert_eq!(be16(&f[36..38]), 50000);
            &f[34..end]
        }
        fn setup() -> (Network, Scratch, Device, usize) {
            let mut n = Network::new(config());
            n.tcp = Tcp::new(0x12345678, 0);
            let h = n.socket.listen(80).unwrap();
            let mut d = Device::new(Vec::new());
            d.rx = None;
            (n, Scratch::new(), d, h)
        }
        fn inject(n: &mut Network, s: &mut Scratch, d: &mut Device, f: Vec<u8>, now: u32) {
            d.rx = Some(f);
            n.poll(d, s, now).unwrap();
        }
        #[test]
        fn network_snapshot_follows_wire_state_and_is_a_nonconsuming_copy() {
            use rv2k_api::{
                event,
                net::{LINK_UP, PEER_PRESENT, VERSION},
            };
            let (mut n, mut s, mut d, h) = setup();
            let initial = n.info();
            assert_eq!(
                (initial.version, initial.state, initial.flags),
                (VERSION, State::Closed, 0)
            );
            assert_eq!((initial.local_ip, initial.local_port), (config().ip, 80));
            assert_eq!((initial.peer_ip, initial.peer_port), ([0; 4], 0));
            n.poll(&mut d, &mut s, 0).unwrap();
            assert_eq!((n.info().state, n.info().flags), (State::Listen, LINK_UP));
            inject(&mut n, &mut s, &mut d, frame(999, 0, SYN, &[], &[], &[]), 0);
            let syn = validate(d.out.last().unwrap());
            let next = u32::from_be_bytes(syn[4..8].try_into().unwrap()).wrapping_add(1);
            assert_eq!(n.info().state, State::SynReceived);
            inject(
                &mut n,
                &mut s,
                &mut d,
                frame(1000, next, ACK, &[], &[], &[]),
                1,
            );
            let ready = n.socket.readiness();
            let connected = n.info();
            assert_eq!(connected.state, State::Established);
            assert_eq!(connected.flags, LINK_UP | PEER_PRESENT);
            assert_eq!((connected.peer_ip, connected.peer_port), (HOST_IP, 50000));
            assert_eq!((connected.received, connected.transmitted), (2, 1));
            assert_eq!(n.socket.readiness(), ready);
            assert_ne!(n.socket.take_events() & event::NET_CONNECTED, 0);
            n.socket
                .send_with(h, 3, |out| {
                    out.copy_from_slice(b"abc");
                    Ok(())
                })
                .unwrap();
            n.poll(&mut d, &mut s, 2).unwrap();
            n.poll(&mut d, &mut s, 102).unwrap();
            assert_eq!(n.info().retransmissions, 1);
            inject(
                &mut n,
                &mut s,
                &mut d,
                frame(1000, 0, RST, &[], &[], &[]),
                103,
            );
            let closed = n.info();
            assert_eq!((closed.state, closed.flags), (State::Closed, LINK_UP));
            assert_eq!(
                (closed.peer_ip, closed.peer_port, closed.resets),
                ([0; 4], 0, 1)
            );
            n.stats.dropped = u16::MAX;
            n.stats.bad_checksum = 4;
            n.stats.fragments = 5;
            n.stats.io_errors = 6;
            let stats = n.info();
            assert_eq!(
                (
                    stats.dropped,
                    stats.bad_checksum,
                    stats.fragments,
                    stats.io_errors
                ),
                (65535, 4, 5, 6)
            );
            assert_eq!(
                (connected.state, connected.received, connected.resets),
                (State::Established, 2, 0)
            );
        }
        #[test]
        fn tcp_wire_busy_maximal_headers_odd_payload_partial_ack_and_streaming_checksum() {
            let (mut n, mut s, mut d, h) = setup();
            d.busy = true;
            let peer_seq = u32::MAX - 1;
            let mut options = vec![1; 40];
            options[..4].copy_from_slice(&[2, 4, 0, 128]);
            inject(
                &mut n,
                &mut s,
                &mut d,
                frame(peer_seq, 0, SYN, &[], &options, &[0; 40]),
                0,
            );
            assert!(d.out.is_empty());
            assert_eq!(d.releases, 1);
            assert_eq!(n.tcp.state(), State::SynReceived);
            d.busy = false;
            n.poll(&mut d, &mut s, 1).unwrap();
            let syn = validate(&d.out[0]);
            assert_eq!(syn[13], SYN | ACK);
            assert_eq!(&syn[20..24], &[2, 4, 0, 128]);
            let server_next = u32::from_be_bytes(syn[4..8].try_into().unwrap()).wrapping_add(1);
            let client_next = peer_seq.wrapping_add(1);
            inject(
                &mut n,
                &mut s,
                &mut d,
                frame(client_next, server_next, ACK, &[], &[], &[]),
                10,
            );
            assert_eq!(n.tcp.state(), State::Established);
            assert_eq!(d.out.len(), 1);
            let payload: Vec<u8> = (0..99).map(|i| i as u8).collect();
            n.socket
                .send_with(h, payload.len(), |b| {
                    b.copy_from_slice(&payload);
                    Ok(())
                })
                .unwrap();
            n.poll(&mut d, &mut s, 10).unwrap();
            let data = validate(&d.out[1]);
            assert_eq!(data[13], ACK | PSH);
            assert_eq!(&data[20..], &payload);
            inject(
                &mut n,
                &mut s,
                &mut d,
                frame(
                    client_next,
                    server_next.wrapping_add(33),
                    ACK,
                    &[],
                    &[],
                    &[],
                ),
                11,
            );
            n.poll(&mut d, &mut s, 111).unwrap();
            let retrans = validate(d.out.last().unwrap());
            assert_eq!(&retrans[20..], &payload[33..]);
            // A corruption beyond the 128-byte receive capacity must still fail
            // the full TCP checksum, without publishing the valid prefix.
            let big = vec![0x77; 1000];
            let good = frame(
                client_next,
                server_next.wrapping_add(99),
                ACK,
                &big,
                &[],
                &[],
            );
            let mut bad = good.clone();
            *bad.last_mut().unwrap() ^= 1;
            let count = d.out.len();
            inject(&mut n, &mut s, &mut d, bad, 112);
            assert_eq!(n.socket.rx_len(), 0);
            assert_eq!(d.out.len(), count);
            inject(&mut n, &mut s, &mut d, good, 113);
            assert_eq!(n.socket.rx_len(), 128);
            let ack = validate(d.out.last().unwrap());
            assert_eq!(
                u32::from_be_bytes(ack[8..12].try_into().unwrap()),
                client_next.wrapping_add(128)
            );
            assert_eq!(be16(&ack[14..16]), 0);
            assert_eq!(n.stats.bad_checksum, 1);
            assert_eq!(n.stats.io_errors, 0);
            assert_eq!(n.socket.tx_len(), 0);
        }
        #[test]
        fn tcp_wire_rejects_malformed_options_offsets_reserved_bits_and_checksum_before_state() {
            let mut cases = Vec::new();
            let good = frame(99, 0, SYN, &[], &[], &[]);
            for (offset, value) in [(46, 0xf0), (46, 0x51), (34, 0), (36, 0)] {
                let mut f = good.clone();
                f[offset] = value;
                if offset == 34 {
                    f[35] = 0;
                }
                if offset == 36 {
                    f[37] = 0;
                }
                fix_tcp(&mut f);
                cases.push(f);
            }
            let mut bad = good.clone();
            bad[50] ^= 1;
            cases.push(bad);
            for options in [
                &[30, 1, 0, 0][..],
                &[2, 3, 1, 0][..],
                &[2, 4, 0, 0][..],
                &[2, 4, 0, 128, 2, 4, 0, 64][..],
            ] {
                cases.push(frame(99, 0, SYN, &[], options, &[]));
            }
            for f in cases {
                let (mut n, mut s, mut d, _) = setup();
                inject(&mut n, &mut s, &mut d, f, 0);
                assert_eq!(n.tcp.state(), State::Listen);
                assert!(d.out.is_empty());
                assert_eq!(d.releases, 1);
                assert_eq!(n.stats.dropped, 1);
                assert_eq!(n.stats.io_errors, 0);
            }
            let (mut n, mut s, mut d, _) = setup();
            inject(
                &mut n,
                &mut s,
                &mut d,
                frame(99, 0, SYN, &[], &[30, 4, 1, 2, 1, 0, 0, 0], &[]),
                0,
            );
            assert_eq!(n.tcp.state(), State::SynReceived);
            validate(&d.out[0]);
        }
        #[test]
        fn link_down_defers_receive_and_pending_control_transmission() {
            let (mut n, mut s, mut d, _) = setup();
            d.link_up = false;
            d.rx = Some(frame(99, 0, SYN, &[], &[], &[]));
            n.poll(&mut d, &mut s, 0).unwrap();
            assert_eq!(d.releases, 0);
            assert!(d.out.is_empty());
            d.link_up = true;
            d.busy = true;
            n.poll(&mut d, &mut s, 1).unwrap();
            assert_eq!(d.releases, 1);
            assert!(d.out.is_empty());
            d.link_up = false;
            d.busy = false;
            n.poll(&mut d, &mut s, 2).unwrap();
            assert!(d.out.is_empty());
            d.link_up = true;
            n.poll(&mut d, &mut s, 3).unwrap();
            assert_eq!(validate(&d.out[0])[13], SYN | ACK);
        }
        #[test]
        fn tcp_wire_closed_port_reset_and_tx_error_leave_a_retryable_syn() {
            let (mut n, mut s, mut d, _) = setup();
            let mut f = frame(99, 123, ACK, &[], &[], &[]);
            f[36..38].copy_from_slice(&81u16.to_be_bytes());
            fix_tcp(&mut f);
            inject(&mut n, &mut s, &mut d, f, 0);
            let reset = &d.out[0][34..54];
            assert_eq!(reset[13], RST);
            assert_eq!(&reset[4..8], &123u32.to_be_bytes());
            let (mut n, mut s, mut d, _) = setup();
            d.fail_write = true;
            d.rx = Some(frame(99, 0, SYN, &[], &[], &[]));
            assert_eq!(n.poll(&mut d, &mut s, 0), Err(Error::Io));
            assert_eq!(d.releases, 1);
            assert_eq!(d.aborts, 1);
            assert!(d.out.is_empty());
            d.fail_write = false;
            n.poll(&mut d, &mut s, 1).unwrap();
            assert_eq!(validate(&d.out[0])[13], SYN | ACK);
        }
        #[test]
        fn wire_rst_challenges_an_in_window_sequence_and_retires_the_exact_connection() {
            let (mut n, mut s, mut d, h) = setup();
            inject(&mut n, &mut s, &mut d, frame(99, 0, SYN, &[], &[], &[]), 0);
            let syn = validate(&d.out[0]);
            let next = u32::from_be_bytes(syn[4..8].try_into().unwrap()).wrapping_add(1);
            inject(
                &mut n,
                &mut s,
                &mut d,
                frame(100, next, ACK, b"abc", &[], &[]),
                1,
            );
            assert_eq!(n.socket.rx_len(), 3);
            let count = d.out.len();
            inject(
                &mut n,
                &mut s,
                &mut d,
                frame(104, next, RST | ACK, &[], &[], &[]),
                2,
            );
            assert_eq!(n.tcp.state(), State::Established);
            assert_eq!(n.socket.rx_len(), 3);
            assert_eq!(d.out.len(), count + 1);
            let challenge = validate(d.out.last().unwrap());
            assert_eq!(challenge[13], ACK);
            assert_eq!(&challenge[8..12], &103u32.to_be_bytes());
            inject(
                &mut n,
                &mut s,
                &mut d,
                frame(103, next, RST | ACK, &[], &[], &[]),
                3,
            );
            assert_eq!(n.tcp.state(), State::Closed);
            assert_eq!(d.out.len(), count + 1);
            assert_eq!(n.socket.recv_with(h, 8, |_| Ok(())), Err(Error::Closed));
            assert_ne!(n.socket.listen(80).unwrap(), h);
            assert_eq!(n.socket.recv_with(h, 8, |_| Ok(())), Err(Error::BadFd));
            assert_eq!(d.releases, 4);
        }
        #[test]
        fn wire_syn_retry_exhaustion_reports_timeout_and_allows_a_new_listener() {
            let (mut n, mut s, mut d, h) = setup();
            inject(&mut n, &mut s, &mut d, frame(99, 0, SYN, &[], &[], &[]), 0);
            let first = validate(&d.out[0]).to_vec();
            let mut now = 0;
            let mut interval = 100;
            for _ in 0..8 {
                now += interval;
                n.poll(&mut d, &mut s, now).unwrap();
                assert_eq!(validate(d.out.last().unwrap()), first);
                interval = (interval * 2).min(6000);
            }
            assert_eq!(d.out.len(), 9);
            n.poll(&mut d, &mut s, now + interval).unwrap();
            assert_eq!(n.tcp.state(), State::Closed);
            assert_eq!(d.out.len(), 9);
            assert_eq!(n.socket.recv_with(h, 8, |_| Ok(())), Err(Error::Timeout));
            assert_ne!(n.socket.listen(80).unwrap(), h);
            inject(
                &mut n,
                &mut s,
                &mut d,
                frame(999, 0, SYN, &[], &[], &[]),
                now + interval + 1,
            );
            assert_eq!(n.tcp.state(), State::SynReceived);
            assert_eq!(validate(d.out.last().unwrap())[13], SYN | ACK);
            assert_eq!(d.releases, 2);
        }
    }
}
