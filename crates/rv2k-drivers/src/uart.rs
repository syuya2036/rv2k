use rv2k_api::syscall::Error;
pub trait UartTx {
    fn try_put(&mut self, byte: u8) -> Result<(), Error>;
}

/// Fixed ring with partial writes. A disconnected/stalled device cannot block
/// the kernel; poll attempts no more than its caller-supplied byte budget.
pub struct Console<T> {
    device: T,
    bytes: [u8; 32],
    head: u8,
    len: u8,
}
impl<T: UartTx> Console<T> {
    pub const fn new(device: T) -> Self {
        Self {
            device,
            bytes: [0; 32],
            head: 0,
            len: 0,
        }
    }
    pub fn write(&mut self, bytes: &[u8]) -> Result<usize, Error> {
        if self.len == 32 && !bytes.is_empty() {
            return Err(Error::Again);
        }
        let count = bytes.len().min(32 - self.len as usize);
        for (i, byte) in bytes[..count].iter().enumerate() {
            self.bytes[(self.head as usize + self.len as usize + i) % 32] = *byte;
        }
        self.len += count as u8;
        Ok(count)
    }
    pub fn poll(&mut self, budget: u8) -> Result<usize, Error> {
        let mut sent = 0;
        while self.len != 0 && sent < budget as usize {
            match self.device.try_put(self.bytes[self.head as usize]) {
                Ok(()) => {
                    self.head = (self.head + 1) % 32;
                    self.len -= 1;
                    sent += 1;
                }
                Err(Error::Again) => break,
                Err(error) => return Err(error),
            }
        }
        Ok(sent)
    }
    pub fn pending(&self) -> usize {
        self.len as usize
    }
    /// Copy directly into a contiguous free ring span. Publish only after a
    /// successful copy; no additional kernel-stack buffer is needed.
    pub fn write_from(
        &mut self,
        len: usize,
        copy: impl FnOnce(&mut [u8]) -> Result<(), Error>,
    ) -> Result<usize, Error> {
        if len == 0 {
            return Ok(0);
        }
        if self.len == 32 {
            return Err(Error::Again);
        }
        let tail = (self.head as usize + self.len as usize) % 32;
        let count = len.min(32 - self.len as usize).min(32 - tail);
        copy(&mut self.bytes[tail..tail + count])?;
        self.len += count as u8;
        Ok(count)
    }
    pub fn device_mut(&mut self) -> &mut T {
        &mut self.device
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;
    struct Device {
        ready: bool,
        fail: bool,
        bytes: Vec<u8>,
    }
    impl UartTx for Device {
        fn try_put(&mut self, b: u8) -> Result<(), Error> {
            if self.fail {
                return Err(Error::Io);
            }
            if !self.ready {
                return Err(Error::Again);
            }
            self.bytes.push(b);
            Ok(())
        }
    }
    #[test]
    fn partial_write_backpressure_wrap_bounded_poll_and_retry() {
        let mut c = Console::new(Device {
            ready: false,
            fail: false,
            bytes: Vec::new(),
        });
        let bytes: [u8; 40] = core::array::from_fn(|i| i as u8);
        assert_eq!(c.write(&bytes), Ok(32));
        assert_eq!(c.write(b"x"), Err(Error::Again));
        assert_eq!(c.poll(32), Ok(0));
        c.device_mut().ready = true;
        assert_eq!(c.poll(7), Ok(7));
        assert_eq!(c.pending(), 25);
        assert_eq!(c.write(&bytes[32..]), Ok(7));
        c.device_mut().fail = true;
        assert_eq!(c.poll(1), Err(Error::Io));
        assert_eq!(c.pending(), 32);
        c.device_mut().fail = false;
        assert_eq!(c.poll(32), Ok(32));
        assert_eq!(c.device_mut().bytes, &bytes[..39]);
    }
}

#[cfg(test)]
mod copy_tests {
    use super::*;
    struct Sink;
    impl UartTx for Sink {
        fn try_put(&mut self, _: u8) -> Result<(), Error> {
            Ok(())
        }
    }
    #[test]
    fn failed_user_copy_never_publishes_bytes() {
        let mut c = Console::new(Sink);
        assert_eq!(
            c.write_from(8, |out| {
                out[0] = 0xee;
                Err(Error::Fault)
            }),
            Err(Error::Fault)
        );
        assert_eq!(c.pending(), 0);
        assert_eq!(c.poll(32), Ok(0));
        assert_eq!(
            c.write_from(40, |out| {
                out.fill(1);
                Ok(())
            }),
            Ok(32)
        );
        assert_eq!(
            c.write_from(1, |_| panic!("must not read while full")),
            Err(Error::Again)
        );
        assert_eq!(c.poll(1), Ok(1));
        assert_eq!(
            c.write_from(8, |out| {
                assert_eq!(out.len(), 1);
                out[0] = 2;
                Ok(())
            }),
            Ok(1)
        );
    }
}
