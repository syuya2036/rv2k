//! Socket syscall boundary, independent of concrete protocol/device storage.
use crate::{ipc::Mailbox, memory::UserMemory, syscall::Outcome, Action, Scheduler};
use rv2k_api::{
    event,
    syscall::{Error, Number},
    TaskId, TaskState,
};

pub trait SocketService {
    fn info(&self) -> Result<rv2k_api::net::NetInfo, Error> {
        Err(Error::NoSys)
    }
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
            | Number::NetInfo
            | Number::EventWait
    ) {
        return None;
    }
    let Some(caller) = scheduler.current() else {
        return Some(Outcome::Complete(Error::Invalid.code()));
    };
    let result = if matches!(call, Number::EventWait) {
        if args[0] > u32::MAX as usize || args[1] > u32::MAX as usize || args[2] > 1 {
            Err(Error::Invalid)
        } else {
            let level = mailbox.readiness(caller)
                | if caller == TaskId::Application {
                    socket.readiness()
                } else {
                    0
                };
            match scheduler.wait_event_call(args, level) {
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
    if call == Number::NetInfo {
        if args[1] != rv2k_api::net::BYTES || args[2] != 0 {
            return Err(Error::Invalid);
        }
        let mut info = socket.info()?;
        if scheduler.info(TaskId::Network).state == TaskState::Exited {
            info.flags |= rv2k_api::net::NETWORK_STOPPED;
        }
        return info.with_bytes(|bytes| memory.copy_out(args[0], bytes, 2).map(|_| 0));
    }
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
mod snapshot_tests {
    use super::*;
    use crate::{
        memory::{MemoryMap, Region},
        services::{dispatch_runtime, InfoSource, NoConsole, RuntimeServices},
        Selection,
    };
    use core::cell::Cell;
    use rv2k_api::net::{NetInfo, TcpState, BYTES};
    fn snapshot() -> NetInfo {
        NetInfo {
            version: 1,
            state: TcpState::Closed,
            flags: 0,
            local_ip: [192, 168, 123, 50],
            peer_ip: [0; 4],
            local_port: 23,
            peer_port: 0,
            received: 1,
            transmitted: 2,
            dropped: 3,
            bad_checksum: 4,
            fragments: 5,
            io_errors: 6,
            retransmissions: 7,
            resets: 8,
        }
    }
    struct Output([u8; BYTES]);
    impl UserMemory for Output {
        fn copy_out(
            &mut self,
            address: usize,
            bytes: &[u8],
            alignment: usize,
        ) -> Result<(), Error> {
            MemoryMap {
                stack: Region {
                    start: 0x1000,
                    end: 0x1020,
                },
                application: Region { start: 0, end: 0 },
                flash: Region { start: 0, end: 0 },
            }
            .validate(address, bytes.len(), alignment, true)?;
            self.0.copy_from_slice(bytes);
            Ok(())
        }
    }
    struct Source {
        reads: Cell<usize>,
        available: bool,
    }
    impl SocketService for Source {
        fn info(&self) -> Result<NetInfo, Error> {
            self.reads.set(self.reads.get() + 1);
            if self.available {
                Ok(snapshot())
            } else {
                Err(Error::NoSys)
            }
        }
        fn listen(&mut self, _: u16) -> Result<usize, Error> {
            panic!("snapshot must not listen")
        }
        fn recv(
            &mut self,
            _: &mut impl UserMemory,
            _: usize,
            _: usize,
            _: usize,
        ) -> Result<usize, Error> {
            panic!("snapshot must not recv")
        }
        fn send(
            &mut self,
            _: &mut impl UserMemory,
            _: usize,
            _: usize,
            _: usize,
        ) -> Result<usize, Error> {
            panic!("snapshot must not send")
        }
        fn close(&mut self, _: usize) -> Result<(), Error> {
            panic!("snapshot must not close")
        }
        fn readiness(&self) -> u32 {
            panic!("snapshot must not consume socket readiness")
        }
    }
    #[test]
    fn both_dispatchers_keep_absolute_expiry_and_relative_readiness_semantics() {
        for full in [false, true] {
            let mut scheduler = Scheduler::new();
            scheduler.initialize([0x100, 0x200]);
            scheduler.select_next();
            scheduler.advance_ticks(501);
            let mut mailbox = Mailbox::new();
            let mut output = Output([0xcc; BYTES]);
            let mut source = Source {
                reads: Cell::new(0),
                available: false,
            };
            let mut call = |args| {
                if full {
                    dispatch_runtime(
                        &mut scheduler,
                        &mut mailbox,
                        &mut output,
                        4,
                        args,
                        RuntimeServices::<NoConsole, _> {
                            console: None,
                            socket: Some(&mut source),
                            info: InfoSource::None,
                        },
                    )
                } else {
                    dispatch(&mut scheduler, &mailbox, &mut output, 4, args, &mut source).unwrap()
                }
            };
            for args in [
                [event::NET_RX as usize, 500, 1],
                [event::NET_RX as usize, 501, 1],
            ] {
                assert!(matches!(call(args), Outcome::Complete(n) if n == Error::Timeout.code()));
            }
            assert!(matches!(call([1, 1, 2]), Outcome::Complete(n) if n == Error::Invalid.code()));
            assert!(
                matches!(call([event::MSG_SPACE as usize, 0, 0]), Outcome::Complete(n) if n == event::MSG_SPACE as isize)
            );
            assert!(matches!(
                call([event::NET_RX as usize, 502, 1]),
                Outcome::Switch(Action::Block)
            ));
            assert_eq!(output.0, [0xcc; BYTES]);
            assert_eq!(source.reads.get(), 0);
        }
    }
    fn invoke(
        s: &mut Scheduler,
        mailbox: &mut Mailbox,
        out: &mut Output,
        service: &mut Source,
        args: [usize; 3],
        full: bool,
    ) -> isize {
        let result = if full {
            dispatch_runtime(
                s,
                mailbox,
                out,
                14,
                args,
                RuntimeServices::<NoConsole, _> {
                    console: None,
                    socket: Some(service),
                    info: InfoSource::None,
                },
            )
        } else {
            dispatch(s, mailbox, out, 14, args, service).unwrap()
        };
        match result {
            Outcome::Complete(n) => n,
            _ => panic!("snapshot must not switch"),
        }
    }
    #[test]
    fn snapshot_copy_both_dispatchers_owners_and_pointer_failures() {
        for full in [false, true] {
            let mut s = Scheduler::new();
            s.initialize([0x100, 0x200]);
            s.select_next();
            let mut mailbox = Mailbox::new();
            let mut source = Source {
                reads: Cell::new(0),
                available: true,
            };
            let mut out = Output([0xa5; BYTES]);
            for (args, code) in [
                ([0x1000, 31, 0], -1),
                ([0x1000, 33, 0], -1),
                ([0x1000, 32, 1], -1),
            ] {
                assert_eq!(
                    invoke(&mut s, &mut mailbox, &mut out, &mut source, args, full),
                    code
                );
                assert_eq!(out.0, [0xa5; BYTES]);
            }
            assert_eq!(source.reads.get(), 0);
            for address in [0x1001, 0x1002, 0x0ffe, usize::MAX, usize::MAX - 1] {
                assert_eq!(
                    invoke(
                        &mut s,
                        &mut mailbox,
                        &mut out,
                        &mut source,
                        [address, 32, 0],
                        full
                    ),
                    -8
                );
                assert_eq!(out.0, [0xa5; BYTES]);
            }
            for id in [TaskId::Network, TaskId::Application] {
                assert_eq!(s.current(), Some(id));
                assert_eq!(
                    invoke(
                        &mut s,
                        &mut mailbox,
                        &mut out,
                        &mut source,
                        [0x1000, 32, 0],
                        full
                    ),
                    0
                );
                assert_eq!(out.0, snapshot().encode());
                s.suspend(0x200, Action::Yield);
                s.select_next();
            }
            assert_eq!(s.wait_event(event::DRIVER_POLL, 0, 0), Err(Error::Timeout));
            // A stopped network remains observable; this read does not require ownership.
            s.suspend(0x100, Action::Exit(0));
            assert_eq!(s.select_next(), Selection::Run(TaskId::Application, 0x200));
            assert_eq!(
                invoke(
                    &mut s,
                    &mut mailbox,
                    &mut out,
                    &mut source,
                    [0x1000, 32, 0],
                    full
                ),
                0
            );
            let mut stopped = snapshot();
            stopped.flags |= rv2k_api::net::NETWORK_STOPPED;
            assert_eq!(out.0, stopped.encode());
            out.0.fill(0xa5);
            source.available = false;
            assert_eq!(
                invoke(
                    &mut s,
                    &mut mailbox,
                    &mut out,
                    &mut source,
                    [0x1000, 32, 0],
                    full
                ),
                -2
            );
            assert_eq!(out.0, [0xa5; BYTES]);
        }
    }
    #[test]
    fn snapshot_unavailable_without_network_service() {
        let mut s = Scheduler::new();
        s.initialize([0x100, 0x200]);
        s.select_next();
        let mut mailbox = Mailbox::new();
        let mut out = Output([0xa5; BYTES]);
        let result = dispatch_runtime(
            &mut s,
            &mut mailbox,
            &mut out,
            14,
            [0x1000, 32, 0],
            RuntimeServices::<NoConsole, Source> {
                console: None,
                socket: None,
                info: InfoSource::None,
            },
        );
        assert!(matches!(result, Outcome::Complete(-2)));
        assert_eq!(out.0, [0xa5; BYTES]);
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
