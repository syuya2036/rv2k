//! Full service dispatcher. The Phase 2 task-only dispatcher is retained for its
//! independent ABI regression probe.
use crate::{ipc::Mailbox, memory::UserMemory, syscall::Outcome, Action, Scheduler};
use rv2k_api::{
    syscall::{Error, Number},
    TaskId,
};

pub trait ConsoleWrite {
    fn write_user(
        &mut self,
        memory: &mut impl UserMemory,
        address: usize,
        len: usize,
    ) -> Result<usize, Error>;
}
pub struct DeviceServices<'a, C> {
    pub console: &'a mut C,
    pub info: Option<&'a rv2k_api::mem::MemInfo>,
}
/// All enabled services share one decode and one event-wait implementation.
pub struct RuntimeServices<'a, C, S> {
    pub console: Option<&'a mut C>,
    pub socket: Option<&'a mut S>,
    pub info: InfoSource<'a>,
}
pub enum InfoSource<'a> {
    None,
    Snapshot(&'a rv2k_api::mem::MemInfo),
    Live(fn() -> rv2k_api::mem::MemInfo),
}
impl InfoSource<'_> {
    fn copy(
        &self,
        memory: &mut impl UserMemory,
        address: usize,
        len: usize,
    ) -> Result<usize, Error> {
        if matches!(self, Self::None) {
            return Err(Error::NoSys);
        }
        if len != 24 {
            return Err(Error::Invalid);
        }
        let live;
        let info = match self {
            Self::None => unreachable!(),
            Self::Snapshot(info) => *info,
            Self::Live(read) => {
                live = read();
                &live
            }
        };
        info.with_bytes(|bytes| memory.copy_out(address, bytes, 2))
            .map(|_| 0)
    }
}
pub struct NoConsole;
impl ConsoleWrite for NoConsole {
    fn write_user(&mut self, _: &mut impl UserMemory, _: usize, _: usize) -> Result<usize, Error> {
        Err(Error::NoSys)
    }
}
struct NoSocket;
impl crate::socket::SocketService for NoSocket {
    fn listen(&mut self, _: u16) -> Result<usize, Error> {
        Err(Error::NoSys)
    }
    fn recv(
        &mut self,
        _: &mut impl UserMemory,
        _: usize,
        _: usize,
        _: usize,
    ) -> Result<usize, Error> {
        Err(Error::NoSys)
    }
    fn send(
        &mut self,
        _: &mut impl UserMemory,
        _: usize,
        _: usize,
        _: usize,
    ) -> Result<usize, Error> {
        Err(Error::NoSys)
    }
    fn close(&mut self, _: usize) -> Result<(), Error> {
        Err(Error::NoSys)
    }
    fn readiness(&self) -> u32 {
        0
    }
}
pub fn dispatch_with_devices(
    scheduler: &mut Scheduler,
    mailbox: &mut Mailbox,
    memory: &mut impl UserMemory,
    number: usize,
    args: [usize; 3],
    devices: DeviceServices<'_, impl ConsoleWrite>,
) -> Outcome {
    dispatch_runtime(
        scheduler,
        mailbox,
        memory,
        number,
        args,
        RuntimeServices::<_, NoSocket> {
            console: Some(devices.console),
            socket: None,
            info: devices.info.map_or(InfoSource::None, InfoSource::Snapshot),
        },
    )
}
pub fn dispatch(
    scheduler: &mut Scheduler,
    mailbox: &mut Mailbox,
    memory: &mut impl UserMemory,
    number: usize,
    args: [usize; 3],
) -> Outcome {
    dispatch_with_info(scheduler, mailbox, memory, number, args, None)
}
pub fn dispatch_with_info(
    scheduler: &mut Scheduler,
    mailbox: &mut Mailbox,
    memory: &mut impl UserMemory,
    number: usize,
    args: [usize; 3],
    info: Option<&rv2k_api::mem::MemInfo>,
) -> Outcome {
    dispatch_runtime(
        scheduler,
        mailbox,
        memory,
        number,
        args,
        RuntimeServices::<NoConsole, NoSocket> {
            console: None,
            socket: None,
            info: info.map_or(InfoSource::None, InfoSource::Snapshot),
        },
    )
}
pub fn dispatch_runtime(
    scheduler: &mut Scheduler,
    mailbox: &mut Mailbox,
    memory: &mut impl UserMemory,
    number: usize,
    args: [usize; 3],
    services: RuntimeServices<'_, impl ConsoleWrite, impl crate::socket::SocketService>,
) -> Outcome {
    let Some(caller) = scheduler.current() else {
        return Outcome::Complete(Error::Invalid.code());
    };
    let result = match Number::decode(number) {
        Some(Number::Yield) => return Outcome::Switch(Action::Yield),
        Some(Number::GetPid) => Ok(caller.index()),
        Some(Number::TaskExit) => match i16::try_from(args[0] as isize) {
            Ok(code) => return Outcome::Switch(Action::Exit(code)),
            Err(_) => Err(Error::Invalid),
        },
        Some(Number::TaskInfo) => crate::syscall::task_info(scheduler, memory, args),
        Some(Number::MemInfo) => services.info.copy(memory, args[0], args[1]),
        Some(Number::Sleep) => match u32::try_from(args[0])
            .ok()
            .and_then(|n| scheduler.sleep(n).ok())
        {
            Some(action) => return Outcome::Switch(action),
            None => Err(Error::Invalid),
        },
        Some(Number::Uptime) => {
            if args[1] != 8 {
                Err(Error::Invalid)
            } else {
                memory
                    .copy_out(
                        args[0],
                        &scheduler.uptime().to_le_bytes(),
                        core::mem::align_of::<u64>(),
                    )
                    .map(|_| 0)
            }
        }
        Some(Number::EventWait) => {
            if args[0] > u32::MAX as usize || args[1] > u32::MAX as usize {
                Err(Error::Invalid)
            } else {
                let level = mailbox.readiness(caller)
                    | if caller == TaskId::Application {
                        services.socket.as_ref().map_or(0, |s| s.readiness())
                    } else {
                        0
                    };
                match scheduler.wait_event(args[0] as u32, args[1] as u32, level) {
                    Ok(Some(bits)) => Ok(bits as usize),
                    Ok(None) => return Outcome::Switch(Action::Block),
                    Err(e) => Err(e),
                }
            }
        }
        Some(Number::EventSignal) => match TaskId::from_index(args[0]) {
            Some(id) if args[1] <= u32::MAX as usize => {
                scheduler.signal(id, args[1] as u32).map(|_| 0)
            }
            _ => Err(Error::Invalid),
        },
        Some(Number::Write) => match services.console {
            Some(c) if args[0] == 1 => c.write_user(memory, args[1], args[2]),
            Some(_) => Err(Error::BadFd),
            None => Err(Error::NoSys),
        },
        Some(
            call @ (Number::SocketListen
            | Number::SocketRecv
            | Number::SocketSend
            | Number::SocketClose
            | Number::NetInfo),
        ) => match services.socket {
            Some(socket) => crate::socket::operation(scheduler, memory, call, args, socket, caller),
            None => Err(Error::NoSys),
        },
        Some(Number::MessageSend) => match TaskId::from_index(args[0]) {
            Some(id) => mailbox.send(scheduler, memory, id, args[1], args[2]),
            None => Err(Error::Invalid),
        },
        Some(Number::MessageRecv) => mailbox.recv(scheduler, memory, args[0], args[1]),
        _ => Err(Error::NoSys),
    };
    Outcome::Complete(match result {
        Ok(n) => n as isize,
        Err(e) => e.code(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rv2k_api::event;
    struct Memory([u8; 64]);
    impl UserMemory for Memory {
        fn copy_out(
            &mut self,
            address: usize,
            bytes: &[u8],
            alignment: usize,
        ) -> Result<(), Error> {
            if address != 0x1000 || !address.is_multiple_of(alignment) {
                return Err(Error::Fault);
            }
            self.0[..bytes.len()].copy_from_slice(bytes);
            Ok(())
        }
        fn copy_in(&mut self, address: usize, bytes: &mut [u8]) -> Result<(), Error> {
            if address != 0x1000 {
                return Err(Error::Fault);
            }
            bytes.copy_from_slice(&self.0[..bytes.len()]);
            Ok(())
        }
    }
    #[test]
    fn syscall_timer_event_and_message_routes() {
        let mut s = Scheduler::new();
        s.initialize([0x100, 0x200]);
        s.select_next();
        let mut m = Mailbox::new();
        let mut mem = Memory([0; 64]);
        s.advance_ticks(0xffff_ffff);
        s.advance_ticks(2);
        assert!(matches!(
            dispatch(&mut s, &mut m, &mut mem, 3, [0x1000, 8, 0]),
            Outcome::Complete(0)
        ));
        assert_eq!(
            u64::from_le_bytes(mem.0[..8].try_into().unwrap()),
            0x1_0000_0001
        );
        for (number, args, result) in [
            (3, [0x1000, 7, 0], -1),
            (3, [0x1001, 8, 0], -8),
            (1, [0x8000_0000, 0, 0], -1),
            (4, [0, 1, 0], -1),
            (5, [2, 1, 0], -1),
            (5, [0, 1 << 31, 0], -1),
            (6, [0; 3], -2),
        ] {
            assert!(
                matches!(dispatch(&mut s, &mut m, &mut mem, number, args), Outcome::Complete(v) if v == result)
            );
        }
        assert!(matches!(
            dispatch(&mut s, &mut m, &mut mem, 1, [0; 3]),
            Outcome::Switch(Action::Yield)
        ));
        assert!(
            matches!(dispatch(&mut s, &mut m, &mut mem, 4, [event::DRIVER_POLL as usize,0,0]), Outcome::Complete(v) if v == event::DRIVER_POLL as isize)
        );
        assert!(matches!(
            dispatch(&mut s, &mut m, &mut mem, 15, [0, 0x1000, 8]),
            Outcome::Complete(0)
        ));
        assert!(
            matches!(dispatch(&mut s, &mut m, &mut mem, 4, [event::MSG_AVAILABLE as usize,0,0]), Outcome::Complete(v) if v == event::MSG_AVAILABLE as isize)
        );
        mem.0.fill(0);
        assert!(matches!(
            dispatch(&mut s, &mut m, &mut mem, 16, [0x1000, 8, 0]),
            Outcome::Complete(8)
        ));
        assert_eq!(
            u64::from_le_bytes(mem.0[..8].try_into().unwrap()),
            0x1_0000_0001
        );
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    use rv2k_api::mem::MemInfo;
    struct Output([u8; 24]);
    impl UserMemory for Output {
        fn copy_out(
            &mut self,
            address: usize,
            bytes: &[u8],
            alignment: usize,
        ) -> Result<(), Error> {
            if address != 0x1000 || alignment != 2 {
                return Err(Error::Fault);
            }
            self.0.copy_from_slice(bytes);
            Ok(())
        }
    }
    #[test]
    fn snapshot_serialization_version_size_and_faults() {
        let mut s = Scheduler::new();
        s.initialize([0x100, 0x200]);
        s.select_next();
        let mut m = Mailbox::new();
        let mut output = Output([0xcc; 24]);
        let info = MemInfo {
            version: 1,
            static_bytes: 768,
            kernel_reserved: 256,
            task_reserved: 256,
            used: [104, 104, 204],
            guard_failures: 0,
            flash_bytes: 8096,
            ram_limit: 2048,
            flash_limit: 16384,
            reserved: 0,
        };
        info.with_bytes(|bytes| assert_eq!(bytes, &info.encode()));
        assert!(matches!(
            dispatch_with_info(
                &mut s,
                &mut m,
                &mut output,
                13,
                [0x1000, 24, 0],
                Some(&info)
            ),
            Outcome::Complete(0)
        ));
        assert_eq!(&output.0[..4], &[1, 0, 0, 3]);
        assert_eq!(&output.0[16..22], &[0xa0, 0x1f, 0, 8, 0, 0x40]);
        for (args, expected) in [
            ([0x1000, 23, 0], -1),
            ([0x1001, 24, 0], -8),
            ([usize::MAX, 24, 0], -8),
        ] {
            assert!(
                matches!(dispatch_with_info(&mut s,&mut m,&mut output,13,args,Some(&info)),Outcome::Complete(v) if v==expected)
            );
        }
    }
    #[test]
    fn live_snapshot_runs_only_for_a_valid_memory_info_request() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        static READS: AtomicUsize = AtomicUsize::new(0);
        fn read() -> MemInfo {
            READS.fetch_add(1, Ordering::Relaxed);
            MemInfo {
                version: 1,
                static_bytes: 768,
                kernel_reserved: 256,
                task_reserved: 256,
                used: [80; 3],
                guard_failures: 0,
                flash_bytes: 8192,
                ram_limit: 2048,
                flash_limit: 16384,
                reserved: 0,
            }
        }
        let mut s = Scheduler::new();
        s.initialize([0x100, 0x200]);
        s.select_next();
        let mut mailbox = Mailbox::new();
        let mut out = Output([0; 24]);
        for (number, args, expected) in [
            (2, [0; 3], 0),
            (17, [0; 3], -2),
            (13, [0x1000, 23, 0], -1),
            (13, [0x1000, 24, 0], 0),
        ] {
            assert!(
                matches!(dispatch_runtime(&mut s,&mut mailbox,&mut out,number,args,
                RuntimeServices::<NoConsole,NoSocket>{console:None,socket:None,info:InfoSource::Live(read)}),Outcome::Complete(n)if n==expected)
            );
            if number != 13 || args[1] != 24 {
                assert_eq!(READS.load(Ordering::Relaxed), 0);
            }
        }
        assert_eq!(READS.load(Ordering::Relaxed), 1);
        assert_eq!(&out.0[..4], &[1, 0, 0, 3]);
    }
}

#[cfg(test)]
mod console_tests {
    use super::*;
    struct Input;
    impl UserMemory for Input {
        fn copy_out(&mut self, _: usize, _: &[u8], _: usize) -> Result<(), Error> {
            Err(Error::Fault)
        }
        fn copy_in(&mut self, address: usize, out: &mut [u8]) -> Result<(), Error> {
            if address != 0x1000 {
                return Err(Error::Fault);
            }
            out.fill(0x5a);
            Ok(())
        }
    }
    struct Console;
    impl ConsoleWrite for Console {
        fn write_user(
            &mut self,
            m: &mut impl UserMemory,
            address: usize,
            len: usize,
        ) -> Result<usize, Error> {
            let mut out = [0; 32];
            let count = len.min(out.len());
            m.copy_in(address, &mut out[..count])?;
            Ok(count)
        }
    }
    #[test]
    fn console_fd_copy_error_and_partial_acceptance() {
        let mut s = Scheduler::new();
        s.initialize([0x100, 0x200]);
        s.select_next();
        let mut mailbox = Mailbox::new();
        let mut memory = Input;
        let mut console = Console;
        for (args, expected) in [([0, 0x1000, 8], -3), ([1, 0, 8], -8), ([1, 0x1000, 64], 32)] {
            assert!(
                matches!(dispatch_with_devices(&mut s,&mut mailbox,&mut memory,6,args,
                DeviceServices{console:&mut console,info:None}),Outcome::Complete(value)if value==expected)
            );
        }
    }
}
