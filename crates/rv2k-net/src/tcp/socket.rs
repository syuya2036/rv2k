use rv2k_api::{event, syscall::Error};

pub const CAPACITY: usize = 128;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Phase {
    Closed,
    Listening,
    Connected,
    Terminal,
}

/// Fixed syscall mailbox. Only the network task changes connection/EOF state;
/// syscalls copy bytes or publish requests, without retaining user pointers.
pub struct Socket {
    rx: [u8; CAPACITY],
    tx: [u8; CAPACITY],
    rx_len: u8,
    tx_len: u8,
    generation: u16,
    retired: u16,
    port: u16,
    phase: Phase,
    error: Option<Error>,
    eof: bool,
    close: bool,
    listen: bool,
    window_update: bool,
    events: u32,
}

impl Default for Socket {
    fn default() -> Self {
        Self::new()
    }
}
impl Socket {
    pub const fn new() -> Self {
        Self {
            rx: [0; CAPACITY],
            tx: [0; CAPACITY],
            rx_len: 0,
            tx_len: 0,
            generation: 1,
            retired: 0,
            port: 0,
            phase: Phase::Closed,
            error: None,
            eof: false,
            close: false,
            listen: false,
            window_update: false,
            events: 0,
        }
    }
    fn handle(&self) -> usize {
        // Low bit is the only slot; the remaining bits identify its incarnation.
        (self.generation as usize) << 1
    }
    fn validate(&self, handle: usize) -> Result<(), Error> {
        if self.phase == Phase::Terminal && handle == (self.retired as usize) << 1 {
            return Err(self.error.unwrap_or(Error::Closed));
        }
        if handle != self.handle() || matches!(self.phase, Phase::Closed | Phase::Terminal) {
            Err(Error::BadFd)
        } else {
            Ok(())
        }
    }
    pub fn listen(&mut self, port: u16) -> Result<usize, Error> {
        if port == 0 {
            return Err(Error::Invalid);
        }
        match self.phase {
            Phase::Connected => return Err(Error::Busy),
            Phase::Listening => {
                return if self.port == port {
                    Ok(self.handle())
                } else {
                    Err(Error::Busy)
                }
            }
            _ => {}
        }
        self.port = port;
        self.rx_len = 0;
        self.tx_len = 0;
        self.error = None;
        self.eof = false;
        self.close = false;
        self.listen = true;
        self.window_update = false;
        self.events = 0;
        self.phase = Phase::Listening;
        Ok(self.handle())
    }
    pub fn send_with(
        &mut self,
        handle: usize,
        len: usize,
        copy: impl FnOnce(&mut [u8]) -> Result<(), Error>,
    ) -> Result<usize, Error> {
        self.validate(handle)?;
        if self.close {
            return Err(Error::Closed);
        }
        if self.phase != Phase::Connected {
            return Err(Error::Again);
        }
        if len == 0 {
            return Ok(0);
        }
        if self.tx_len != 0 {
            return Err(Error::Again);
        }
        let count = len.min(CAPACITY);
        copy(&mut self.tx[..count])?;
        self.tx_len = count as u8;
        Ok(count)
    }
    pub fn recv_with(
        &mut self,
        handle: usize,
        capacity: usize,
        copy: impl FnOnce(&[u8]) -> Result<(), Error>,
    ) -> Result<usize, Error> {
        self.validate(handle)?;
        if capacity == 0 {
            return Ok(0);
        }
        let count = capacity.min(self.rx_len as usize);
        if count == 0 {
            return if self.eof { Ok(0) } else { Err(Error::Again) };
        }
        copy(&self.rx[..count])?;
        self.rx.copy_within(count..self.rx_len as usize, 0);
        self.rx_len -= count as u8;
        self.window_update = true;
        Ok(count)
    }
    pub fn close(&mut self, handle: usize) -> Result<(), Error> {
        self.validate(handle)?;
        self.close = true;
        Ok(())
    }
    pub fn readiness(&self) -> u32 {
        let mut bits = 0;
        if self.phase == Phase::Connected {
            bits |= event::NET_CONNECTED;
            if self.tx_len == 0 && !self.close {
                bits |= event::NET_TX_READY;
            }
        }
        if self.rx_len != 0 || self.eof {
            bits |= event::NET_RX;
        }
        if self.phase == Phase::Terminal {
            bits |= event::NET_CLOSED;
        }
        bits
    }
    pub fn take_events(&mut self) -> u32 {
        core::mem::take(&mut self.events)
    }
    pub fn port(&self) -> u16 {
        self.port
    }
    pub fn rx_len(&self) -> usize {
        self.rx_len as usize
    }
    pub fn tx_len(&self) -> usize {
        self.tx_len as usize
    }

    // The following operations belong exclusively to the protocol task.
    pub(super) fn take_listen(&mut self) -> bool {
        core::mem::take(&mut self.listen)
    }
    pub(super) fn listening(&self) -> bool {
        self.phase == Phase::Listening && !self.close
    }
    pub(super) fn closing(&self) -> bool {
        self.close
    }
    pub(super) fn take_window_update(&mut self) -> bool {
        core::mem::take(&mut self.window_update)
    }
    pub(super) fn connected(&mut self) {
        self.phase = Phase::Connected;
        self.events |= event::NET_CONNECTED | event::NET_TX_READY;
    }
    pub(super) fn receive_with(
        &mut self,
        len: usize,
        copy: impl FnOnce(&mut [u8]) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let start = self.rx_len as usize;
        if self.eof || len > CAPACITY - start {
            return Err(Error::MessageSize);
        }
        copy(&mut self.rx[start..start + len])?;
        self.rx_len += len as u8;
        if len != 0 {
            self.events |= event::NET_RX;
        }
        Ok(())
    }
    pub(super) fn eof(&mut self) {
        self.eof = true;
        self.events |= event::NET_RX;
    }
    pub(super) fn tx(&self, len: usize) -> &[u8] {
        &self.tx[..len]
    }
    pub(super) fn acked(&mut self, count: usize) {
        assert!(count <= self.tx_len as usize);
        self.tx.copy_within(count..self.tx_len as usize, 0);
        self.tx_len -= count as u8;
        if self.tx_len == 0 {
            self.events |= event::NET_TX_READY;
        }
    }
    pub(super) fn terminal(&mut self, error: Error) {
        self.phase = Phase::Terminal;
        self.error = Some(error);
        self.retired = self.generation;
        self.generation = self.generation.wrapping_add(1).max(1);
        self.rx_len = 0;
        self.tx_len = 0;
        self.listen = false;
        self.events |= event::NET_CLOSED;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copies_publish_atomically_buffers_hold_until_ack_and_handles_change() {
        let mut s = Socket::new();
        let h = s.listen(80).unwrap();
        assert_eq!(s.listen(80), Ok(h));
        assert_eq!(s.listen(23), Err(Error::Busy));
        assert_eq!(s.send_with(h, 1, |_| Ok(())), Err(Error::Again));
        assert!(s.take_listen());
        s.connected();
        assert_eq!(
            s.send_with(h, 200, |b| {
                b.fill(7);
                Err(Error::Fault)
            }),
            Err(Error::Fault)
        );
        assert_eq!(s.tx_len(), 0);
        assert_eq!(
            s.send_with(h, 200, |b| {
                b.fill(9);
                Ok(())
            }),
            Ok(128)
        );
        assert_eq!(s.send_with(h, 1, |_| Ok(())), Err(Error::Again));
        s.acked(65);
        assert_eq!(s.tx(63), &[9; 63]);
        assert_eq!(s.send_with(h, 1, |_| Ok(())), Err(Error::Again));
        s.acked(63);
        assert_ne!(s.readiness() & event::NET_TX_READY, 0);
        assert_eq!(
            s.receive_with(128, |b| {
                b.fill(3);
                Err(Error::Io)
            }),
            Err(Error::Io)
        );
        assert_eq!(s.rx_len(), 0);
        s.receive_with(128, |b| {
            b.fill(4);
            Ok(())
        })
        .unwrap();
        assert_eq!(s.recv_with(h, 64, |_| Err(Error::Fault)), Err(Error::Fault));
        assert_eq!(s.rx_len(), 128);
        s.eof();
        assert_eq!(
            s.recv_with(h, 65, |b| {
                assert_eq!(b, &[4; 65]);
                Ok(())
            }),
            Ok(65)
        );
        assert_eq!(
            s.recv_with(h, 65, |b| {
                assert_eq!(b, &[4; 63]);
                Ok(())
            }),
            Ok(63)
        );
        assert_eq!(s.recv_with(h, 1, |_| panic!()), Ok(0));
        assert!(s.take_window_update());
        s.terminal(Error::Timeout);
        assert_eq!(s.recv_with(h, 1, |_| panic!()), Err(Error::Timeout));
        let next = s.listen(80).unwrap();
        assert_ne!(h, next);
        assert_eq!(s.close(h), Err(Error::BadFd));
        s.connected();
        s.close(next).unwrap();
        assert_eq!(s.send_with(next, 1, |_| panic!()), Err(Error::Closed));
    }
}
