//! Streaming, polling ENC28J60 backend. No MCU-sized Ethernet frame arrays.
use crate::{
    network::{DeviceStatus, NetworkDevice, RxFrame, TxStatus, MAX_FRAME},
    spi::SpiBus,
};
use rv2k_api::syscall::Error;
const RX_END: u16 = 0x17ff;
const TX_START: u16 = 0x1800;
const ECON1: u8 = 0x1f;
const ECON2: u8 = 0x1e;
const EIR: u8 = 0x1c;
const EPKTCNT: u8 = 0x39;
const EREVID: u8 = 0x72;
const MIREGADR: u8 = 0xd4;
const MICMD: u8 = 0xd2;
const MISTAT: u8 = 0xea;
const MIRDL: u8 = 0xd8;
const MIWRL: u8 = 0xd6;

#[derive(Clone, Copy, PartialEq)]
enum Tx {
    Idle,
    Building,
    Wire,
}
pub struct Enc28j60<S> {
    spi: S,
    bank: u8,
    ready: bool,
    next_rx: u16,
    active: Option<RxFrame>,
    next_packet: u16,
    generation: u32,
    tx: Tx,
    tx_len: u16,
    tx_written: u16,
    deadline: u32,
}
impl<S: SpiBus> Enc28j60<S> {
    pub const fn new(spi: S) -> Self {
        Self {
            spi,
            bank: 0xff,
            ready: false,
            next_rx: 0,
            active: None,
            next_packet: 0,
            generation: 0,
            tx: Tx::Idle,
            tx_len: 0,
            tx_written: 0,
            deadline: 0,
        }
    }
    pub fn into_bus(self) -> S {
        self.spi
    }
    fn transaction<R>(&mut self, f: impl FnOnce(&mut S) -> Result<R, Error>) -> Result<R, Error> {
        // Even a failed select is followed by deassertion. A transport failure
        // invalidates cached state: callers must reset before reusing the NIC.
        let result = match self.spi.select(true) {
            Ok(()) => f(&mut self.spi),
            Err(e) => Err(e),
        };
        let end = self.spi.select(false);
        let result = result.and_then(|value| end.map(|_| value));
        if result.is_err() {
            self.ready = false;
            self.bank = 0xff;
        }
        result
    }
    fn bit(&mut self, address: u8, mask: u8, set: bool) -> Result<(), Error> {
        self.transaction(|s| {
            s.transfer((if set { 0x80 } else { 0xa0 }) | (address & 31))?;
            s.transfer(mask)?;
            Ok(())
        })
    }
    fn bank(&mut self, reg: u8) -> Result<(), Error> {
        if reg & 31 >= 0x1b {
            return Ok(());
        }
        let bank = (reg >> 5) & 3;
        if self.bank != bank {
            self.bit(ECON1, 3, false)?;
            if bank != 0 {
                self.bit(ECON1, bank, true)?;
            }
            self.bank = bank;
        }
        Ok(())
    }
    fn read(&mut self, reg: u8) -> Result<u8, Error> {
        self.bank(reg)?;
        self.transaction(|s| {
            s.transfer(reg & 31)?;
            if reg & 0x80 != 0 {
                s.transfer(0)?;
            }
            s.transfer(0)
        })
    }
    fn write(&mut self, reg: u8, value: u8) -> Result<(), Error> {
        self.bank(reg)?;
        self.transaction(|s| {
            s.transfer(0x40 | (reg & 31))?;
            s.transfer(value)?;
            Ok(())
        })
    }
    fn word(&mut self, reg: u8, value: u16) -> Result<(), Error> {
        self.write(reg, value as u8)?;
        self.write(reg + 1, (value >> 8) as u8)
    }
    fn mii_ready(&mut self) -> Result<(), Error> {
        for _ in 0..32 {
            if self.read(MISTAT)? & 1 == 0 {
                return Ok(());
            }
        }
        self.ready = false;
        Err(Error::Timeout)
    }
    fn phy_write(&mut self, reg: u8, value: u16) -> Result<(), Error> {
        self.write(MIREGADR, reg)?;
        self.word(MIWRL, value)?;
        self.mii_ready()
    }
    fn phy_read(&mut self, reg: u8) -> Result<u16, Error> {
        self.write(MIREGADR, reg)?;
        self.write(MICMD, 1)?;
        let ready = self.mii_ready();
        self.write(MICMD, 0)?;
        ready?;
        let lo = self.read(MIRDL)?;
        let hi = self.read(MIRDL + 1)?;
        Ok(u16::from_le_bytes([lo, hi]))
    }
    pub fn reset(&mut self, mac: [u8; 6]) -> Result<u8, Error> {
        self.ready = false;
        self.bank = 0xff;
        self.active = None;
        self.tx = Tx::Idle;
        self.generation = self.generation.wrapping_add(1);
        self.transaction(|s| {
            s.transfer(0xff)?;
            Ok(())
        })?;
        self.spi.delay_us(2000); // Errata: CLKRDY is stale after soft reset.
        self.bank = 0;
        let revision = self.read(EREVID)?;
        // B1/B4 have an asynchronous slow-SPI MAC erratum. Software SPI is
        // supported only on B5/B7; absent/floating devices fail explicitly.
        if revision != 5 && revision != 6 {
            return Err(Error::Io);
        }
        self.bit(ECON2, 0x80, true)?;
        self.word(0x08, 0)?;
        self.word(0x0a, RX_END)?;
        self.word(0x0c, RX_END)?;
        self.word(0x04, TX_START)?;
        self.write(0x38, 0xa1)?; // unicast+broadcast+CRC filter
        self.write(0xc0, 1)?;
        self.write(0xc2, 0x32)?;
        self.write(0xc3, 0x40)?;
        self.write(0xc4, 0x12)?;
        self.write(0xc6, 0x12)?;
        self.write(0xc7, 0x0c)?;
        self.word(0xca, 1518)?;
        for (reg, value) in [0xe4, 0xe5, 0xe2, 0xe3, 0xe0, 0xe1].into_iter().zip(mac) {
            self.write(reg, value)?;
        }
        self.phy_write(0, 0)?;
        self.phy_write(0x10, 0x0100)?; // half duplex, no local loopback
        self.next_rx = 0;
        self.bit(ECON1, 4, true)?;
        self.ready = true;
        Ok(revision)
    }
    fn ensure(&self) -> Result<(), Error> {
        if self.ready {
            Ok(())
        } else {
            Err(Error::Closed)
        }
    }
    fn release_next(&mut self, next: u16) -> Result<(), Error> {
        self.word(0x0c, if next == 0 { RX_END } else { next - 1 })?;
        self.bit(ECON2, 0x40, true)?;
        self.next_rx = next;
        self.active = None;
        Ok(())
    }
    fn read_ring(&mut self, address: u16, out: &mut [u8]) -> Result<(), Error> {
        let mut position = address as usize;
        let mut offset = 0;
        while offset < out.len() {
            let count = (RX_END as usize + 1 - position).min(out.len() - offset);
            self.word(0, position as u16)?;
            self.transaction(|s| {
                s.transfer(0x3a)?;
                for byte in &mut out[offset..offset + count] {
                    *byte = s.transfer(0)?;
                }
                Ok(())
            })?;
            position = 0;
            offset += count;
        }
        Ok(())
    }
    fn reset_tx(&mut self) -> Result<(), Error> {
        self.bit(ECON1, 8, false)?;
        self.bit(ECON1, 0x80, true)?;
        self.bit(ECON1, 0x80, false)?;
        self.bit(EIR, 0x0a, false)
    }
}
impl<S: SpiBus> NetworkDevice for Enc28j60<S> {
    fn poll(&mut self, now: u32) -> Result<DeviceStatus, Error> {
        self.ensure()?;
        let mut tx = if self.tx == Tx::Idle {
            TxStatus::Idle
        } else {
            TxStatus::Busy
        };
        if self.tx == Tx::Wire {
            let flags = self.read(EIR)?;
            let control = self.read(ECON1)?;
            if flags & 2 != 0 || (control & 8 != 0 && now.wrapping_sub(self.deadline) < 0x8000_0000)
            {
                self.reset_tx()?;
                self.tx = Tx::Idle;
                tx = TxStatus::Failed;
            } else if control & 8 == 0 {
                tx = if flags & 8 != 0 {
                    TxStatus::Complete
                } else {
                    TxStatus::Failed
                };
                self.reset_tx()?;
                self.tx = Tx::Idle;
            }
        }
        Ok(DeviceStatus {
            link_up: self.phy_read(0x11)? & 0x0400 != 0,
            rx_pending: self.read(EPKTCNT)? != 0,
            tx,
        })
    }
    fn rx_begin(&mut self) -> Result<Option<RxFrame>, Error> {
        self.ensure()?;
        if self.active.is_some() {
            return Err(Error::Busy);
        }
        if self.read(EPKTCNT)? == 0 {
            return Ok(None);
        }
        let mut header = [0; 6];
        self.read_ring(self.next_rx, &mut header)?;
        let next = u16::from_le_bytes([header[0], header[1]]);
        let len = u16::from_le_bytes([header[2], header[3]]);
        if next > RX_END || next & 1 != 0 || next == self.next_rx {
            self.ready = false;
            return Err(Error::Io);
        }
        if !(64..=1518).contains(&len) || header[4] & 0x80 == 0 || header[4] & 0x70 != 0 {
            self.release_next(next)?;
            return Err(Error::Io);
        }
        self.generation = self.generation.wrapping_add(1);
        let frame = RxFrame {
            token: self.generation,
            len: len - 4,
        };
        self.next_packet = next;
        self.active = Some(frame);
        Ok(Some(frame))
    }
    fn rx_read_at(&mut self, frame: RxFrame, offset: usize, out: &mut [u8]) -> Result<(), Error> {
        self.ensure()?;
        if self.active != Some(frame) {
            return Err(Error::Invalid);
        }
        if offset
            .checked_add(out.len())
            .is_none_or(|end| end > frame.len as usize)
        {
            return Err(Error::Invalid);
        }
        let address = (self.next_rx as usize + 6 + offset) % (RX_END as usize + 1);
        self.read_ring(address as u16, out)
    }
    fn rx_release(&mut self, frame: RxFrame) -> Result<(), Error> {
        self.ensure()?;
        if self.active != Some(frame) {
            return Err(Error::Invalid);
        }
        self.release_next(self.next_packet)
    }
    fn tx_begin(&mut self, len: usize) -> Result<(), Error> {
        self.ensure()?;
        if self.tx != Tx::Idle {
            return Err(Error::Again);
        }
        if !(14..=MAX_FRAME).contains(&len) {
            return Err(Error::MessageSize);
        }
        if self.read(ECON1)? & 8 != 0 {
            return Err(Error::Again);
        }
        self.word(0x02, TX_START)?;
        self.transaction(|s| {
            s.transfer(0x7a)?;
            s.transfer(0)?;
            Ok(())
        })?;
        self.tx = Tx::Building;
        self.tx_len = len as u16;
        self.tx_written = 0;
        Ok(())
    }
    fn tx_write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.ensure()?;
        if self.tx != Tx::Building {
            return Err(Error::Invalid);
        }
        if bytes.len() > self.tx_len as usize - self.tx_written as usize {
            return Err(Error::MessageSize);
        }
        self.transaction(|s| {
            s.transfer(0x7a)?;
            for byte in bytes {
                s.transfer(*byte)?;
            }
            Ok(())
        })?;
        self.tx_written += bytes.len() as u16;
        Ok(())
    }
    fn tx_commit(&mut self, now: u32) -> Result<(), Error> {
        self.ensure()?;
        if self.tx != Tx::Building || self.tx_written != self.tx_len {
            return Err(Error::Invalid);
        }
        self.reset_tx()?;
        self.word(0x04, TX_START)?;
        self.word(0x06, TX_START + self.tx_len)?;
        self.bit(ECON1, 8, true)?;
        self.tx = Tx::Wire;
        self.deadline = now.wrapping_add(100);
        Ok(())
    }
    fn tx_abort(&mut self) -> Result<(), Error> {
        self.ensure()?;
        if self.tx == Tx::Wire {
            self.reset_tx()?;
        }
        self.tx = Tx::Idle;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{boxed::Box, vec::Vec};
    struct Model {
        regs: [[u8; 32]; 4],
        phy: [u16; 32],
        ram: Box<[u8; 8192]>,
        selected: bool,
        command: Option<u8>,
        index: usize,
        delay: u32,
        transfers: usize,
        fail_after: Option<usize>,
        mii_stuck: bool,
        revision: u8,
        selects: Vec<bool>,
    }
    impl Model {
        fn new() -> Self {
            let mut m = Self {
                regs: [[0; 32]; 4],
                phy: [0; 32],
                ram: Box::new([0; 8192]),
                selected: false,
                command: None,
                index: 0,
                delay: 0,
                transfers: 0,
                fail_after: None,
                mii_stuck: false,
                revision: 6,
                selects: Vec::new(),
            };
            m.reset();
            m
        }
        fn reset(&mut self) {
            self.regs = [[0; 32]; 4];
            self.regs[0][ECON2 as usize] = 0x80;
            self.regs[3][0x12] = self.revision;
            self.regs[0][0x1d] = 1;
            self.phy = [0; 32];
            self.phy[0x11] = 0x0400;
        }
        fn bank(&self, reg: u8) -> usize {
            if reg >= 0x1b {
                0
            } else {
                (self.regs[0][31] & 3) as usize
            }
        }
        fn word(&self, reg: usize) -> usize {
            u16::from_le_bytes([self.regs[0][reg], self.regs[0][reg + 1]]) as usize
        }
        fn set_word(&mut self, reg: usize, value: usize) {
            self.regs[0][reg] = value as u8;
            self.regs[0][reg + 1] = (value >> 8) as u8;
        }
        fn mac(&self, reg: u8) -> bool {
            match self.bank(reg) {
                2 => true,
                3 => reg <= 5 || reg == 0x0a,
                _ => false,
            }
        }
        fn write(&mut self, reg: u8, value: u8, op: u8) {
            let b = self.bank(reg);
            let old = self.regs[b][reg as usize];
            let value = match op {
                0x80 => old | value,
                0xa0 => old & !value,
                _ => value,
            };
            self.regs[b][reg as usize] = value;
            if b == 0 && reg == ECON2 && value & 0x40 != 0 {
                self.regs[1][0x19] = self.regs[1][0x19].saturating_sub(1);
                self.regs[0][ECON2 as usize] &= !0x40;
            }
            if b == 2 && reg == 0x17 {
                let address = self.regs[2][0x14] as usize;
                self.phy[address] = u16::from_le_bytes([self.regs[2][0x16], value]);
            }
            if b == 2 && reg == 0x12 && value & 1 != 0 {
                let value = self.phy[self.regs[2][0x14] as usize].to_le_bytes();
                self.regs[2][0x18] = value[0];
                self.regs[2][0x19] = value[1];
            }
        }
        fn inject(&mut self, start: u16, next: u16, payload: &[u8], status: u8) {
            let len = (payload.len() + 4) as u16;
            let header = [
                next as u8,
                (next >> 8) as u8,
                len as u8,
                (len >> 8) as u8,
                status,
                0,
            ];
            for (i, byte) in header
                .iter()
                .chain(payload.iter())
                .chain([0; 4].iter())
                .enumerate()
            {
                self.ram[(start as usize + i) % (RX_END as usize + 1)] = *byte;
            }
            self.regs[1][0x19] += 1;
        }
    }
    impl SpiBus for Model {
        fn select(&mut self, active: bool) -> Result<(), Error> {
            self.selects.push(active);
            self.selected = active;
            if active {
                self.command = None;
                self.index = 0;
            }
            Ok(())
        }
        fn transfer(&mut self, byte: u8) -> Result<u8, Error> {
            assert!(self.selected);
            if self.fail_after.is_some_and(|limit| self.transfers >= limit) {
                return Err(Error::Io);
            }
            self.transfers += 1;
            let Some(command) = self.command else {
                self.command = Some(byte);
                if byte == 0xff {
                    self.reset();
                }
                return Ok(0);
            };
            self.index += 1;
            if command == 0x3a {
                let ptr = self.word(0);
                let value = self.ram[ptr];
                self.set_word(0, (ptr + 1) % 8192);
                return Ok(value);
            }
            if command == 0x7a {
                let ptr = self.word(2);
                self.ram[ptr] = byte;
                self.set_word(2, (ptr + 1) % 8192);
                return Ok(0);
            }
            let reg = command & 31;
            if command & 0xe0 == 0 {
                if self.mac(reg) && self.index == 1 {
                    return Ok(0xde);
                }
                if self.bank(reg) == 3 && reg == 0x0a {
                    return Ok(self.mii_stuck as u8);
                }
                return Ok(self.regs[self.bank(reg)][reg as usize]);
            }
            self.write(reg, byte, command & 0xe0);
            Ok(0)
        }
        fn delay_us(&mut self, us: u16) {
            self.delay += us as u32;
        }
    }
    fn device() -> Enc28j60<Model> {
        let mut d = Enc28j60::new(Model::new());
        assert_eq!(d.reset([2, 0, 0, 0, 0, 1]), Ok(6));
        d
    }
    #[test]
    fn reset_banks_dummy_mii_mac_mapping_and_link() {
        let mut d = device();
        assert!(d.spi.delay >= 1000);
        assert_eq!(d.spi.word(8), 0);
        assert_eq!(d.spi.word(10), RX_END as usize);
        assert_eq!(d.spi.word(12), RX_END as usize);
        assert_eq!(
            [
                d.spi.regs[3][4],
                d.spi.regs[3][5],
                d.spi.regs[3][2],
                d.spi.regs[3][3],
                d.spi.regs[3][0],
                d.spi.regs[3][1]
            ],
            [2, 0, 0, 0, 0, 1]
        );
        assert_eq!(d.spi.regs[2][2], 0x32);
        assert_eq!(d.spi.phy[0x10], 0x100);
        assert!(d.poll(0).unwrap().link_up);
        d.spi.phy[0x11] = 0;
        assert!(!d.poll(1).unwrap().link_up);
        assert!(d.spi.selects.chunks(2).all(|pair| pair == [true, false]));
    }
    #[test]
    fn rx_stream_wrap_reread_release_and_stale_handles() {
        let mut d = device();
        d.next_rx = RX_END - 3;
        let payload = [0xa5; 128];
        d.spi.inject(d.next_rx, 132, &payload, 0x80);
        let f = d.rx_begin().unwrap().unwrap();
        assert_eq!(f.len, 128);
        assert_eq!(d.rx_begin(), Err(Error::Busy));
        let mut out = [0; 17];
        d.rx_read_at(f, 0, &mut out).unwrap();
        assert_eq!(out, [0xa5; 17]);
        d.rx_read_at(f, 111, &mut out).unwrap();
        assert_eq!(out, [0xa5; 17]);
        assert_eq!(d.rx_read_at(f, 112, &mut out), Err(Error::Invalid));
        assert_eq!(d.rx_read_at(f, usize::MAX, &mut out), Err(Error::Invalid));
        d.rx_release(f).unwrap();
        assert_eq!(d.spi.word(12), 131);
        assert_eq!(d.spi.regs[1][0x19], 0);
        assert_eq!(d.rx_release(f), Err(Error::Invalid));
        assert_eq!(d.rx_read_at(f, 0, &mut out), Err(Error::Invalid));
        d.spi.inject(132, 0, &[1; 60], 0x80);
        let f = d.rx_begin().unwrap().unwrap();
        d.rx_release(f).unwrap();
        assert_eq!(d.spi.word(12), RX_END as usize);
    }
    #[test]
    fn bad_status_and_length_release_but_corrupt_pointer_requires_reset() {
        let mut d = device();
        d.spi.inject(0, 70, &[0; 60], 0x90);
        assert_eq!(d.rx_begin(), Err(Error::Io));
        assert_eq!(d.spi.regs[1][0x19], 0);
        assert_eq!(d.spi.word(12), 69);
        d.spi.inject(70, 140, &[0; 10], 0x80);
        assert_eq!(d.rx_begin(), Err(Error::Io));
        assert_eq!(d.spi.regs[1][0x19], 0);
        d.spi.inject(140, 0x1fff, &[0; 60], 0x80);
        assert_eq!(d.rx_begin(), Err(Error::Io));
        assert_eq!(d.poll(0), Err(Error::Closed));
        assert_eq!(d.reset([2; 6]), Ok(6));
    }
    #[test]
    fn tx_partial_busy_abort_error_and_wrapping_deadline() {
        let mut d = device();
        assert_eq!(d.tx_begin(13), Err(Error::MessageSize));
        d.tx_begin(60).unwrap();
        assert_eq!(d.tx_begin(60), Err(Error::Again));
        d.tx_write(&[1; 20]).unwrap();
        assert_eq!(d.tx_commit(0), Err(Error::Invalid));
        assert_eq!(d.tx_write(&[2; 41]), Err(Error::MessageSize));
        d.tx_write(&[2; 40]).unwrap();
        d.tx_commit(u32::MAX - 50).unwrap();
        assert_eq!(
            &d.spi.ram[TX_START as usize..TX_START as usize + 21],
            &[0].into_iter().chain([1; 20]).collect::<Vec<_>>()[..]
        );
        assert_eq!(d.poll(u32::MAX).unwrap().tx, TxStatus::Busy);
        assert_eq!(d.poll(49).unwrap().tx, TxStatus::Failed);
        assert_eq!(d.spi.regs[0][31] & 8, 0);
        d.tx_begin(60).unwrap();
        d.tx_abort().unwrap();
        d.tx_begin(60).unwrap();
        d.tx_write(&[3; 60]).unwrap();
        d.tx_commit(50).unwrap();
        d.spi.regs[0][EIR as usize] |= 8;
        d.spi.regs[0][ECON1 as usize] &= !8;
        assert_eq!(d.poll(51).unwrap().tx, TxStatus::Complete);
        d.tx_begin(60).unwrap();
        d.tx_write(&[4; 60]).unwrap();
        d.tx_commit(52).unwrap();
        d.spi.regs[0][EIR as usize] |= 2;
        assert_eq!(d.poll(53).unwrap().tx, TxStatus::Failed);
        d.tx_begin(60).unwrap();
        d.tx_write(&[5; 60]).unwrap();
        d.tx_commit(54).unwrap();
        d.tx_abort().unwrap();
        assert_eq!(d.spi.regs[0][ECON1 as usize] & 8, 0);
    }
    #[test]
    fn transport_failure_deasserts_and_poisoned_state_recovers_by_reset() {
        let mut d = device();
        d.spi.fail_after = Some(d.spi.transfers + 1);
        assert_eq!(d.tx_begin(60), Err(Error::Io));
        assert!(!d.spi.selected);
        assert_eq!(d.tx_begin(60), Err(Error::Closed));
        d.spi.fail_after = None;
        d.reset([2; 6]).unwrap();
        assert_eq!(d.tx_begin(60), Ok(()));
        d.spi.mii_stuck = true;
        assert_eq!(d.poll(0), Err(Error::Timeout));
        assert!(!d.spi.selected);
        assert_eq!(d.poll(1), Err(Error::Closed));
    }
    #[test]
    fn absent_and_slow_spi_unsafe_revisions_are_rejected() {
        for revision in [0, 2, 4, 0xff] {
            let mut bus = Model::new();
            bus.revision = revision;
            let mut d = Enc28j60::new(bus);
            assert_eq!(d.reset([2; 6]), Err(Error::Io));
            assert_eq!(d.poll(0), Err(Error::Closed));
        }
    }
    #[test]
    fn full_mtu_streams_without_an_mcu_frame_buffer() {
        let mut d = Enc28j60::new(Model::new());
        d.reset([2; 6]).unwrap();
        let payload: [u8; 1514] = core::array::from_fn(|i| i as u8);
        d.spi.inject(0, 1524, &payload, 0x80);
        let frame = d.rx_begin().unwrap().unwrap();
        assert_eq!(frame.len, 1514);
        let mut scratch = [0; 64];
        let mut offset = 0;
        while offset < payload.len() {
            let count = scratch.len().min(payload.len() - offset);
            d.rx_read_at(frame, offset, &mut scratch[..count]).unwrap();
            assert_eq!(&scratch[..count], &payload[offset..offset + count]);
            offset += count;
        }
        d.rx_release(frame).unwrap();
        assert_eq!(d.tx_begin(1515), Err(Error::MessageSize));
        d.tx_begin(1514).unwrap();
        for chunk in payload.chunks(64) {
            d.tx_write(chunk).unwrap();
        }
        d.tx_commit(0).unwrap();
        assert_eq!(d.spi.word(6), TX_START as usize + 1514);
        assert_eq!(
            &d.spi.ram[TX_START as usize + 1..TX_START as usize + 1515],
            &payload
        );
    }
}
