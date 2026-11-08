//! Streaming TCP wire boundary. Verify all lengths/options/checksum before state.
use crate::{
    be16,
    checksum::Checksum,
    tcp::{Endpoint, Segment, SYN},
    Network, Scratch,
};
use rv2k_api::syscall::Error;
use rv2k_drivers::network::NetworkDevice;

#[inline(always)]
fn be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes[..4].try_into().unwrap())
}
fn pseudo(sum: &mut Checksum, source: &[u8; 4], dest: &[u8; 4], len: usize) {
    sum.update(source);
    sum.update(dest);
    sum.update(&[0, 6]);
    sum.update(&(len as u16).to_be_bytes());
}
impl Network {
    #[inline(never)]
    pub(crate) fn tcp_receive(
        &mut self,
        d: &mut impl NetworkDevice,
        s: &mut Scratch,
    ) -> Result<(), Error> {
        let f = s.reception.frame;
        let start = s.reception.start;
        let len = s.reception.len;
        if len < 20 {
            self.drop_frame();
            return Ok(());
        }
        // Reuse the header scratch from offset zero, even when IP has 40 bytes
        // of options. Combined maximal IP+TCP headers need not coexist in RAM.
        d.rx_read_at(f, start, &mut s.header[..20])?;
        let size = (s.header[12] >> 4) as usize * 4;
        if !(20..=60).contains(&size)
            || size > len
            || s.header[12] & 15 != 0
            || be16(&s.header[..2]) == 0
            || be16(&s.header[2..4]) == 0
        {
            self.drop_frame();
            return Ok(());
        }
        if size > 20 {
            d.rx_read_at(f, start + 20, &mut s.header[20..size])?;
        }
        let mut mss = 536;
        let mut seen_mss = false;
        let mut options = &s.header[20..size];
        while !options.is_empty() {
            let kind = options[0];
            if kind == 0 {
                break;
            }
            if kind == 1 {
                options = &options[1..];
                continue;
            }
            if options.len() < 2 || options[1] < 2 || options[1] as usize > options.len() {
                self.drop_frame();
                return Ok(());
            }
            let size = options[1] as usize;
            let option = &options[..size];
            if kind == 2 {
                if size != 4 || seen_mss || be16(&option[2..4]) == 0 {
                    self.drop_frame();
                    return Ok(());
                }
                if s.header[13] & SYN != 0 {
                    mss = be16(&option[2..4]);
                }
                seen_mss = true;
            }
            options = &options[size..];
        }
        let mut sum = Checksum::new();
        pseudo(&mut sum, &s.reception.packet.peer.ip, &self.config.ip, len);
        if !Self::frame_checksum(d, f, start, len, &mut s.stream, &mut sum)? {
            self.stats.bad_checksum = self.stats.bad_checksum.saturating_add(1);
            self.drop_frame();
            return Ok(());
        }
        s.reception.packet = Segment {
            peer: Endpoint {
                mac: s.reception.packet.peer.mac,
                ip: s.reception.packet.peer.ip,
                port: be16(&s.header[..2]),
            },
            local_port: be16(&s.header[2..4]),
            seq: be32(&s.header[4..8]),
            ack: be32(&s.header[8..12]),
            window: be16(&s.header[14..16]),
            flags: s.header[13],
            len: (len - size) as u16,
            mss,
        };
        s.reception.start = start + size;
        s.reception.valid = true;
        Ok(())
    }

    #[inline(never)]
    pub(crate) fn tcp_transmit(
        &mut self,
        d: &mut impl NetworkDevice,
        s: &mut Scratch,
        now: u32,
    ) -> Result<(), Error> {
        if !self.tcp.prepare(&mut self.socket, now, &mut s.output) {
            return Ok(());
        }
        let out = &s.output;
        let size = if out.syn() { 24 } else { 20 };
        let len = size + out.len as usize;
        match d.tx_begin(34 + len) {
            Ok(()) => {}
            Err(Error::Again) => return Ok(()),
            Err(e) => return Err(e),
        }
        self.ipv4_header(
            &mut s.header,
            &out.peer.mac,
            &out.peer.ip,
            (20 + len) as u16,
            6,
        );
        let h = &mut s.header;
        h[34..34 + size].fill(0);
        h[34..36].copy_from_slice(&out.local_port.to_be_bytes());
        h[36..38].copy_from_slice(&out.peer.port.to_be_bytes());
        h[38..42].copy_from_slice(&out.seq.to_be_bytes());
        h[42..46].copy_from_slice(&out.ack.to_be_bytes());
        h[46] = (size as u8 / 4) << 4;
        h[47] = out.flags;
        h[48..50].copy_from_slice(&out.window.to_be_bytes());
        if out.syn() {
            h[54..58].copy_from_slice(&[2, 4, 0, 128]);
        }
        let payload = self.tcp.payload(&self.socket, out);
        let mut sum = Checksum::new();
        pseudo(&mut sum, &self.config.ip, &out.peer.ip, len);
        sum.update(&h[34..34 + size]);
        sum.update(payload);
        h[50..52].copy_from_slice(&sum.finish().to_be_bytes());
        let result = d
            .tx_write(&h[..34 + size])
            .and_then(|_| d.tx_write(payload))
            .and_then(|_| d.tx_commit(now));
        if result.is_err() {
            let _ = d.tx_abort();
        } else {
            self.tcp.sent(&self.socket, out, now);
            self.stats.transmitted = self.stats.transmitted.saturating_add(1);
        }
        result
    }
}
