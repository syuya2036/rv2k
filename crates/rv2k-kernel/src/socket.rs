//! Socket syscall boundary, independent of concrete protocol/device storage.
use crate::{ipc::Mailbox, memory::UserMemory, syscall::Outcome, Action, Scheduler};
use rv2k_api::{
    event,
    syscall::{Error, Number},
    TaskId, TaskState,
};

pub trait SocketService {
    fn listen(&mut self, port: u16) -> Result<usize, Error>;
    fn recv(
        &mut self,
        memory: &mut impl UserMemory,
        handle: usize,
        address: usize,
        len: usize,
    ) -> Result<usize, Error>;
    fn send(
        &mut self,
        memory: &mut impl UserMemory,
        handle: usize,
        address: usize,
        len: usize,
    ) -> Result<usize, Error>;
    fn close(&mut self, handle: usize) -> Result<(), Error>;
    fn readiness(&self) -> u32;
}

/// Return None for non-socket calls, to keep the existing service dispatcher and
/// optional device adapters intact. No user buffer is borrowed beyond dispatch.
pub fn dispatch(
    scheduler: &mut Scheduler,
    mailbox: &Mailbox,
    memory: &mut impl UserMemory,
    number: usize,
    args: [usize; 3],
    socket: &mut impl SocketService,
) -> Option<Outcome> {
    let call = Number::decode(number)?;
    if !matches!(
        call,
        Number::SocketListen
            | Number::SocketRecv
            | Number::SocketSend
            | Number::SocketClose
            | Number::EventWait
    ) {
        return None;
    }
    let Some(caller) = scheduler.current() else {
        return Some(Outcome::Complete(Error::Invalid.code()));
    };
    let result = if matches!(call, Number::EventWait) {
        if args[0] > u32::MAX as usize || args[1] > u32::MAX as usize {
            Err(Error::Invalid)
        } else {
            let level = mailbox.readiness(caller)
                | if caller == TaskId::Application {
                    socket.readiness()
                } else {
                    0
                };
            match scheduler.wait_event(args[0] as u32, args[1] as u32, level) {
                Ok(Some(bits)) => Ok(bits as usize),
                Ok(None) => return Some(Outcome::Switch(Action::Block)),
                Err(e) => Err(e),
            }
        }
    } else {
        operation(scheduler, memory, call, args, socket, caller)
    };
    Some(Outcome::Complete(match result {
        Ok(n) => n as isize,
        Err(e) => e.code(),
    }))
}

#[inline]
pub(crate) fn operation(
    scheduler: &mut Scheduler,
    memory: &mut impl UserMemory,
    call: Number,
    args: [usize; 3],
    socket: &mut impl SocketService,
    caller: TaskId,
) -> Result<usize, Error> {
    if caller != TaskId::Application {
        Err(Error::BadFd)
    } else if scheduler.info(TaskId::Network).state == TaskState::Exited {
        Err(Error::Closed)
    } else {
        let result = match call {
            Number::SocketListen => u16::try_from(args[0])
                .map_err(|_| Error::Invalid)
                .and_then(|port| socket.listen(port)),
            Number::SocketRecv | Number::SocketSend if args[1].checked_add(args[2]).is_none() => {
                Err(Error::Fault)
            }
            Number::SocketRecv => socket.recv(memory, args[0], args[1], args[2]),
            Number::SocketSend => socket.send(memory, args[0], args[1], args[2]),
            Number::SocketClose => socket.close(args[0]).map(|_| 0),
            _ => unreachable!(),
        };
        if result.is_ok() {
            let _ = scheduler.signal(TaskId::Network, event::DRIVER_POLL);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Memory([u8; 16]);
    impl UserMemory for Memory {
        fn copy_in(&mut self, address: usize, out: &mut [u8]) -> Result<(), Error> {
            if address != 0x1000 || out.len() > 16 {
                return Err(Error::Fault);
            }
            out.copy_from_slice(&self.0[..out.len()]);
            Ok(())
        }
        fn copy_out(&mut self, address: usize, bytes: &[u8], _: usize) -> Result<(), Error> {
            if address != 0x1000 || bytes.len() > 16 {
                return Err(Error::Fault);
            }
            self.0[..bytes.len()].copy_from_slice(bytes);
            Ok(())
        }
    }
    struct Service {
        level: u32,
        bytes: [u8; 16],
        len: usize,
        calls: usize,
    }
    impl SocketService for Service {
        fn listen(&mut self, port: u16) -> Result<usize, Error> {
            self.calls += 1;
            if port == 80 {
                Ok(2)
            } else {
                Err(Error::Invalid)
            }
        }
        fn recv(
            &mut self,
            m: &mut impl UserMemory,
            h: usize,
            a: usize,
            n: usize,
        ) -> Result<usize, Error> {
            self.calls += 1;
            if h != 2 {
                return Err(Error::BadFd);
            }
            let n = n.min(self.len);
            m.copy_out(a, &self.bytes[..n], 1)?;
            self.len -= n;
            Ok(n)
        }
        fn send(
            &mut self,
            m: &mut impl UserMemory,
            h: usize,
            a: usize,
            n: usize,
        ) -> Result<usize, Error> {
            self.calls += 1;
            if h != 2 {
                return Err(Error::BadFd);
            }
            let n = n.min(16);
            m.copy_in(a, &mut self.bytes[..n])?;
            self.len = n;
            Ok(n)
        }
        fn close(&mut self, h: usize) -> Result<(), Error> {
            self.calls += 1;
            if h == 2 {
                Ok(())
            } else {
                Err(Error::BadFd)
            }
        }
        fn readiness(&self) -> u32 {
            self.level
        }
    }
    #[test]
    fn ownership_faults_partial_copy_level_waits_and_network_exit() {
        let mut s = Scheduler::new();
        s.initialize([0x100, 0x200]);
        s.select_next();
        let mailbox = Mailbox::new();
        let mut memory = Memory([0x55; 16]);
        let mut service = Service {
            level: event::NET_RX,
            bytes: [0; 16],
            len: 0,
            calls: 0,
        };
        assert!(matches!(
            dispatch(&mut s, &mailbox, &mut memory, 7, [80, 0, 0], &mut service),
            Some(Outcome::Complete(-3))
        ));
        assert_eq!(service.calls, 0);
        s.suspend(0x100, Action::Yield);
        s.select_next();
        for (number, args, expected) in [
            (7, [80, 0, 0], 2),
            (7, [65536, 0, 0], -1),
            (9, [2, 0x1000, 20], 16),
            (9, [2, usize::MAX, 8], -8),
            (9, [2, 0x1000, usize::MAX], -8),
            (8, [3, 0x1000, 8], -3),
            (8, [2, 0, 8], -8),
            (8, [2, 0x1000, 8], 8),
            (4, [event::NET_RX as usize, 0, 0], event::NET_RX as isize),
            (4, [0, 0, 0], -1),
            (10, [2, 0, 0], 0),
        ] {
            assert!(
                matches!(dispatch(&mut s,&mailbox,&mut memory,number,args,&mut service),Some(Outcome::Complete(v))if v==expected)
            );
        }
        assert!(dispatch(&mut s, &mailbox, &mut memory, 13, [0; 3], &mut service).is_none());
        service.level = 0;
        assert!(matches!(
            dispatch(
                &mut s,
                &mailbox,
                &mut memory,
                4,
                [event::NET_RX as usize, 1, 0],
                &mut service
            ),
            Some(Outcome::Switch(Action::Block))
        ));
        s.suspend(0x200, Action::Block);
        s.select_next();
        s.suspend(0x100, Action::Exit(0));
        s.select_next();
        assert!(matches!(
            dispatch(&mut s, &mailbox, &mut memory, 7, [80, 0, 0], &mut service),
            Some(Outcome::Complete(-7))
        ));
    }
    fn unified(
        s: &mut Scheduler,
        m: &mut Mailbox,
        u: &mut impl UserMemory,
        n: usize,
        a: [usize; 3],
        socket: &mut Service,
    ) -> Option<Outcome> {
        Some(crate::services::dispatch_runtime(
            s,
            m,
            u,
            n,
            a,
            crate::services::RuntimeServices::<crate::services::NoConsole, _> {
                console: None,
                socket: Some(socket),
                info: crate::services::InfoSource::None,
            },
        ))
    }
    #[test]
    fn integrated_dispatch_preserves_socket_ownership_copy_wait_and_exit() {
        let mut s = Scheduler::new();
        s.initialize([0x100, 0x200]);
        s.select_next();
        let mut mailbox = Mailbox::new();
        let mut memory = Memory([0x55; 16]);
        let mut service = Service {
            level: event::NET_RX,
            bytes: [0; 16],
            len: 0,
            calls: 0,
        };
        assert!(matches!(
            unified(
                &mut s,
                &mut mailbox,
                &mut memory,
                7,
                [80, 0, 0],
                &mut service
            ),
            Some(Outcome::Complete(-3))
        ));
        assert_eq!(service.calls, 0);
        s.suspend(0x100, Action::Yield);
        s.select_next();
        for (number, args, expected) in [
            (7, [80, 0, 0], 2),
            (7, [65536, 0, 0], -1),
            (9, [2, 0x1000, 20], 16),
            (9, [2, usize::MAX, 8], -8),
            (9, [2, 0x1000, usize::MAX], -8),
            (8, [3, 0x1000, 8], -3),
            (8, [2, 0, 8], -8),
            (8, [2, 0x1000, 8], 8),
            (4, [event::NET_RX as usize, 0, 0], event::NET_RX as isize),
            (4, [0, 0, 0], -1),
            (10, [2, 0, 0], 0),
        ] {
            assert!(
                matches!(unified(&mut s,&mut mailbox,&mut memory,number,args,&mut service),Some(Outcome::Complete(v))if v==expected)
            );
        }
        assert!(matches!(
            unified(&mut s, &mut mailbox, &mut memory, 13, [0; 3], &mut service),
            Some(Outcome::Complete(-2))
        ));
        service.level = 0;
        assert!(matches!(
            unified(
                &mut s,
                &mut mailbox,
                &mut memory,
                4,
                [event::NET_RX as usize, 1, 0],
                &mut service
            ),
            Some(Outcome::Switch(Action::Block))
        ));
        s.suspend(0x200, Action::Block);
        s.select_next();
        s.suspend(0x100, Action::Exit(0));
        s.select_next();
        assert!(matches!(
            unified(
                &mut s,
                &mut mailbox,
                &mut memory,
                7,
                [80, 0, 0],
                &mut service
            ),
            Some(Outcome::Complete(-7))
        ));
    }
}
