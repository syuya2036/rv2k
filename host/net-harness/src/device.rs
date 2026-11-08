use remu_bus::{Device, DeviceError};
use remu_core::{AccessWidth, SimTime};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};
#[derive(Default)]
pub struct State {
    incoming: VecDeque<Vec<u8>>,
    staging: Vec<u8>,
    declared: usize,
    wire: Option<Vec<u8>>,
    token: u32,
    pub stop: bool,
    pub received: u32,
    pub transmitted: u32,
    pub released: u32,
    done: bool,
}
impl State {
    pub fn empty(&self) -> bool {
        self.incoming.is_empty() && self.wire.is_none()
    }
    pub fn inject(&mut self, frame: Vec<u8>) -> Result<(), String> {
        if frame.len() > 1514
            || self.incoming.iter().map(|f| f.len() + 6).sum::<usize>() + frame.len() + 6 > 6144
        {
            return Err("virtual RX ring full or frame too large".into());
        }
        self.incoming.push_back(frame);
        self.received += 1;
        Ok(())
    }
    pub fn take_tx(&mut self) -> Option<Vec<u8>> {
        let result = self.wire.take();
        if result.is_some() {
            self.done = true;
        }
        result
    }
}
pub struct Nic(pub Rc<RefCell<State>>);
impl Device for Nic {
    fn name(&self) -> &str {
        "rv2k.virtual-ethernet"
    }
    fn read(&mut self, offset: u64, width: AccessWidth, _: SimTime) -> Result<u64, DeviceError> {
        let s = self.0.borrow();
        let value = match (offset, width) {
            (0, AccessWidth::Word) => {
                1 | if !s.incoming.is_empty() { 2 } else { 0 }
                    | if s.wire.is_some() { 4 } else { 0 }
                    | if s.done { 8 } else { 0 }
            }
            (4, AccessWidth::Word) => s.incoming.front().map_or(0, |f| f.len() as u64),
            (8, AccessWidth::Word) => s.token as u64,
            (20, AccessWidth::Word) => s.stop as u64,
            (0x100..=0x6e9, AccessWidth::Byte) => *s
                .incoming
                .front()
                .and_then(|f| f.get(offset as usize - 0x100))
                .ok_or_else(|| DeviceError::new("RX read outside current frame"))?
                as u64,
            _ => return Err(DeviceError::new("invalid virtual NIC read")),
        };
        Ok(value)
    }
    fn write(
        &mut self,
        offset: u64,
        width: AccessWidth,
        value: u64,
        _: SimTime,
    ) -> Result<(), DeviceError> {
        let mut s = self.0.borrow_mut();
        match (offset, width) {
            (12, AccessWidth::Word) if (14..=1514).contains(&value) && s.wire.is_none() => {
                s.declared = value as usize;
                s.staging.clear();
            }
            (16, AccessWidth::Word) => match value {
                1 => {
                    s.incoming
                        .pop_front()
                        .ok_or_else(|| DeviceError::new("double RX release"))?;
                    s.token = s.token.wrapping_add(1);
                    s.released += 1;
                }
                2 if s.declared != 0 && s.staging.len() == s.declared && s.wire.is_none() => {
                    let mut frame = core::mem::take(&mut s.staging);
                    frame.resize(frame.len().max(60), 0);
                    s.wire = Some(frame);
                    s.transmitted += 1;
                    s.declared = 0;
                    s.done = false;
                }
                3 => {
                    s.staging.clear();
                    s.declared = 0;
                    s.wire = None;
                }
                4 => s.done = false,
                _ => return Err(DeviceError::new("invalid virtual NIC command")),
            },
            (0x800..=0xde9, AccessWidth::Byte)
                if offset as usize - 0x800 == s.staging.len() && s.staging.len() < s.declared =>
            {
                s.staging.push(value as u8)
            }
            _ => return Err(DeviceError::new("invalid virtual NIC write")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rv2k_api::syscall::Error;
    use rv2k_drivers::{
        network::{NetworkDevice, TxStatus},
        virtual_nic::{VirtualIo, VirtualNic},
    };
    struct Io(Nic);
    impl VirtualIo for Io {
        fn read(&mut self, offset: usize) -> u32 {
            self.0
                .read(offset as u64, AccessWidth::Word, SimTime::ZERO)
                .unwrap() as u32
        }
        fn write(&mut self, offset: usize, value: u32) {
            self.0
                .write(
                    offset as u64,
                    AccessWidth::Word,
                    value as u64,
                    SimTime::ZERO,
                )
                .unwrap();
        }
        fn read_byte(&mut self, offset: usize) -> u8 {
            self.0
                .read(offset as u64, AccessWidth::Byte, SimTime::ZERO)
                .unwrap() as u8
        }
        fn write_byte(&mut self, offset: usize, value: u8) {
            self.0
                .write(
                    offset as u64,
                    AccessWidth::Byte,
                    value as u64,
                    SimTime::ZERO,
                )
                .unwrap();
        }
    }
    #[test]
    fn guest_driver_and_host_model_enforce_streaming_ownership_and_backpressure() {
        let state = Rc::new(RefCell::new(State::default()));
        let mut d = VirtualNic::from_io(Io(Nic(state.clone())));
        for _ in 0..4 {
            state.borrow_mut().inject(vec![0x42; 1514]).unwrap();
        }
        assert!(state.borrow_mut().inject(vec![0; 1514]).is_err());
        let f = d.rx_begin().unwrap().unwrap();
        assert_eq!(d.rx_begin(), Err(Error::Busy));
        let mut chunk = [0; 64];
        d.rx_read_at(f, 1450, &mut chunk).unwrap();
        assert_eq!(chunk, [0x42; 64]);
        assert_eq!(d.rx_read_at(f, 1451, &mut chunk), Err(Error::Invalid));
        d.rx_release(f).unwrap();
        assert_eq!(d.rx_release(f), Err(Error::Invalid));
        let next = d.rx_begin().unwrap().unwrap();
        assert_ne!(next.token, f.token);
        assert_eq!(d.rx_read_at(f, 0, &mut chunk), Err(Error::Invalid));
        d.tx_begin(14).unwrap();
        assert_eq!(d.tx_commit(0), Err(Error::Invalid));
        assert_eq!(d.tx_write(&[0; 15]), Err(Error::MessageSize));
        d.tx_write(&[0x55; 14]).unwrap();
        d.tx_commit(0).unwrap();
        assert_eq!(d.poll(0).unwrap().tx, TxStatus::Busy);
        assert_eq!(d.tx_begin(14), Err(Error::Again));
        let out = state.borrow_mut().take_tx().unwrap();
        assert_eq!(&out[..14], &[0x55; 14]);
        assert_eq!(&out[14..], &[0; 46]);
        assert_eq!(d.poll(0).unwrap().tx, TxStatus::Complete);
        assert_eq!(d.poll(0).unwrap().tx, TxStatus::Idle);
        d.tx_begin(1514).unwrap();
        d.tx_write(&chunk).unwrap();
        d.tx_abort().unwrap();
        assert!(state.borrow_mut().take_tx().is_none());
        state.borrow_mut().stop = true;
        assert!(d.finished());
    }
}
