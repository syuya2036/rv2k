use crate::{memory::UserMemory, Scheduler};
use rv2k_api::{event, syscall::Error, TaskId, TaskState};

/// One global message. Payload stays in the kernel and is never placed on the
/// kernel stack or shared with network buffers.
pub struct Mailbox {
    payload: [u8; 64],
    destination: Option<TaskId>,
    sender: TaskId,
    len: u8,
}

impl Mailbox {
    pub const fn new() -> Self {
        Self {
            payload: [0; 64],
            destination: None,
            sender: TaskId::Network,
            len: 0,
        }
    }
    pub fn readiness(&self, id: TaskId) -> u32 {
        if self.destination == Some(id) {
            event::MSG_AVAILABLE
        } else if self.destination.is_none() {
            event::MSG_SPACE
        } else {
            0
        }
    }
    pub fn send(
        &mut self,
        scheduler: &mut Scheduler,
        memory: &mut impl UserMemory,
        destination: TaskId,
        address: usize,
        len: usize,
    ) -> Result<usize, Error> {
        if len > 64 {
            return Err(Error::MessageSize);
        }
        if scheduler.info(destination).state == TaskState::Exited {
            return Err(Error::Closed);
        }
        if self.destination.is_some() {
            return Err(Error::Again);
        }
        let sender = scheduler.current().ok_or(Error::Invalid)?;
        if len != 0 {
            memory.copy_in(address, &mut self.payload[..len])?;
        }
        self.len = len as u8;
        self.sender = sender;
        self.destination = Some(destination);
        scheduler.signal(destination, event::MSG_AVAILABLE)?;
        Ok(0)
    }
    pub fn recv(
        &mut self,
        scheduler: &mut Scheduler,
        memory: &mut impl UserMemory,
        address: usize,
        capacity: usize,
    ) -> Result<usize, Error> {
        let caller = scheduler.current().ok_or(Error::Invalid)?;
        if self.destination != Some(caller) {
            return Err(Error::Again);
        }
        let len = self.len as usize;
        if capacity < len {
            return Err(Error::MessageSize);
        }
        if len != 0 {
            memory.copy_out(address, &self.payload[..len], 1)?;
        }
        self.destination = None;
        let _ = scheduler.signal(self.sender, event::MSG_SPACE);
        Ok(len)
    }
    pub fn task_exited(&mut self, scheduler: &mut Scheduler, id: TaskId) {
        if self.destination == Some(id) {
            self.destination = None;
            let _ = scheduler.signal(self.sender, event::MSG_SPACE);
        }
    }
}
impl Default for Mailbox {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Action, Selection};
    struct Bytes([u8; 64]);
    impl UserMemory for Bytes {
        fn copy_out(&mut self, address: usize, bytes: &[u8], _: usize) -> Result<(), Error> {
            if address != 1 {
                return Err(Error::Fault);
            }
            self.0[..bytes.len()].copy_from_slice(bytes);
            Ok(())
        }
        fn copy_in(&mut self, address: usize, bytes: &mut [u8]) -> Result<(), Error> {
            if address != 1 {
                return Err(Error::Fault);
            }
            bytes.copy_from_slice(&self.0[..bytes.len()]);
            Ok(())
        }
    }
    #[test]
    fn full_slot_faults_wakeup_capacity_retry_and_exit_cleanup() {
        use TaskId::{Application as B, Network as A};
        let mut s = Scheduler::new();
        s.initialize([0x100, 0x200]);
        s.select_next();
        let mut m = Mailbox::new();
        let mut bytes = Bytes([0x5a; 64]);
        assert_eq!(
            m.send(&mut s, &mut bytes, B, 1, 65),
            Err(Error::MessageSize)
        );
        assert_eq!(m.send(&mut s, &mut bytes, B, 0, 64), Err(Error::Fault));
        assert_eq!(m.send(&mut s, &mut bytes, B, 1, 64), Ok(0));
        assert_eq!(m.send(&mut s, &mut bytes, B, 1, 1), Err(Error::Again));
        s.wait_event(event::MSG_SPACE, event::FOREVER, m.readiness(A))
            .unwrap();
        s.suspend(0x110, Action::Block);
        assert_eq!(s.select_next(), Selection::Run(B, 0x200));
        assert_eq!(m.recv(&mut s, &mut bytes, 1, 63), Err(Error::MessageSize));
        assert_eq!(m.recv(&mut s, &mut bytes, 0, 64), Err(Error::Fault));
        bytes.0.fill(0);
        assert_eq!(m.recv(&mut s, &mut bytes, 1, 64), Ok(64));
        assert_eq!(bytes.0, [0x5a; 64]);
        assert_eq!(s.take_result(A), Some(event::MSG_SPACE as isize));
        assert_eq!(m.recv(&mut s, &mut bytes, 1, 64), Err(Error::Again));
        assert_eq!(m.send(&mut s, &mut bytes, A, 0, 0), Ok(0));
        s.suspend(0x210, Action::Yield);
        s.select_next();
        assert_eq!(m.recv(&mut s, &mut bytes, 0, 0), Ok(0));
        m.send(&mut s, &mut bytes, B, 1, 1).unwrap();
        s.suspend(0x110, Action::Yield);
        s.select_next();
        s.suspend(0x210, Action::Exit(0));
        m.task_exited(&mut s, B);
        assert_eq!(m.readiness(A), event::MSG_SPACE);
        assert_eq!(m.send(&mut s, &mut bytes, B, 1, 1), Err(Error::Closed));
    }
}
